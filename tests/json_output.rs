//! `--json`: the error taxonomy (#9) and the versioned envelope (#44).
//!
//! Two levels, because the contract has two halves that fail differently.
//!
//! The **library-level** tests below classify errors produced by the real
//! production constructors and validators — `SearchTarget::new`,
//! `ProductTarget::new`, `CategoryId::resolve`, `AppConfig::validate_country`,
//! `SearchTarget::validate` — rather than by hand-built `IherbError`s. That
//! matters: a test that builds `IherbError::InvalidInput` itself and asserts it
//! classifies as `invalid_input` passes whether or not any code in this crate
//! ever produces one, which is exactly the state #9's table was in before this
//! landed. Five of its codes had no producer at all.
//!
//! The **process-level** tests run the built binary. Only a separate process
//! can answer the question `--json` is actually about — whether *stdout* holds
//! one document and nothing else — because the thing that used to break it was
//! a tracing subscriber writing somewhere no in-process assertion looks.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use iherb_cli::app::wants_json;
use iherb_cli::cache::CacheKey;
use iherb_cli::cli::{Section, SortOrder};
use iherb_cli::config::AppConfig;
use iherb_cli::error::{classify_error, ErrorKind, IherbError};
use iherb_cli::fetch::{FetchTarget, Fetched, Provenance};
use iherb_cli::model::{
    Extraction, ProductDetail, ProductSummary, SearchFetch, SearchResult, Source, Strategy,
};
use iherb_cli::output::{
    accounted_json_keys, format_product_json, format_search_json, product_json_keys, Envelope,
    Meta, ProductView, ALWAYS_RENDERED, SCHEMA_VERSION,
};
use serde_json::Value;

// ---------------------------------------------------------------------------
// The taxonomy itself
// ---------------------------------------------------------------------------

/// The exit-code table, as the documentation actually holds it.
///
/// Parsed out of the files a caller reads, not copied into a constant here.
/// The copy is what this test used to be: a second handwritten Rust array
/// beside `ErrorKind`, compared against `ErrorKind`. Changing the README's
/// Cloudflare row from 22 to 25 left it green — so the one thing its name
/// promised, catching documentation drift, was the one thing it could not do.
///
/// Both files are read, because #9 asks for the table in both and a `SKILL.md`
/// that has drifted misleads exactly the agent the table exists for.
fn documented_table(path: &str) -> Vec<(u8, String)> {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("{} is part of this tool's interface: {}", path, e));

    let section = text
        .split("### Exit codes")
        .nth(1)
        .unwrap_or_else(|| panic!("{} no longer documents the exit codes", path));
    // Stop at the next heading, so a later table cannot be read as this one.
    let section = section.split("\n## ").next().unwrap();

    let mut rows = Vec::new();
    for line in section.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 2 {
            continue;
        }
        // The header (`exit`) and the `|---|` separator both fail this, and so
        // does the `0` success row, which names no `error_type`.
        let (Ok(code), Some(error_type)) = (
            cells[0].parse::<u8>(),
            cells[1].strip_prefix('`').and_then(|c| c.strip_suffix('`')),
        ) else {
            continue;
        };
        rows.push((code, error_type.to_string()));
    }

    assert!(
        rows.len() > 5,
        "{}'s exit-code table did not parse; it had {} rows",
        path,
        rows.len()
    );
    rows
}

/// The taxonomy and the documentation of it are one thing, and this is what
/// says so. A caller branches on these numbers and these strings, so changing
/// one is a breaking change to this tool's interface — and a change made in the
/// code but not in the README, or the other way round, is the bug.
#[test]
fn the_exit_code_table_is_what_the_readme_documents() {
    let mut from_code: Vec<(u8, String)> = ErrorKind::ALL
        .iter()
        .map(|k| (k.exit_code(), k.error_type().to_string()))
        .collect();
    from_code.sort_unstable();

    for path in ["README.md", "skills/iherb-agent/SKILL.md"] {
        let mut documented = documented_table(path);
        documented.sort_unstable();
        assert_eq!(
            documented, from_code,
            "{} and ErrorKind disagree about the exit-code table",
            path
        );
    }

    // Every code distinct. A taxonomy whose members share a number is a
    // taxonomy a caller cannot branch on, which is the whole complaint #9 was
    // filed about.
    let mut codes: Vec<u8> = ErrorKind::ALL.iter().map(|k| k.exit_code()).collect();
    codes.sort_unstable();
    let unique = {
        let mut c = codes.clone();
        c.dedup();
        c
    };
    assert_eq!(codes, unique, "two error kinds share an exit code");

    // 0 is success and 1 is "an error happened, no idea which" — the state this
    // whole issue exists to leave behind.
    assert!(codes.iter().all(|c| *c > 1));
}

/// **A documented code must have a reachable producer.** That is the invariant
/// #9 exists to establish, and the one the first round of it missed: five codes
/// were wired and four — `network_error`, `io_error`, `cache_error` and
/// `json_error` — stayed decorative, documented distinctions the code could not
/// make.
///
/// # This test used to be a blocklist, and a blocklist cannot check an invariant
///
/// It asserted that four literal strings were absent from [`ErrorKind`], and
/// nothing else. So the failure it was named for — a code documented in both
/// tables with nothing in the crate that produces it — walked straight past it,
/// as long as the code was not called one of those four things. A fifth
/// decorative code was one variant away, and the test would have stayed green
/// while its name promised otherwise.
///
/// # What it checks now
///
/// Three things, over the crate's own source:
///
///  1. [`ErrorKind::ALL`] holds every variant the enum declares, parsed out of
///     `src/error.rs`. Everything below sweeps `ALL`, so a variant missing from
///     it would be exempt from all of it.
///  2. Every [`IherbError`] variant the enum declares has a sample in
///     [`error_samples`]. Adding a variant without one fails here rather than
///     being silently skipped.
///  3. **Every [`ErrorKind`] is produced.** A kind counts as produced when
///     production code outside `src/error.rs` either names it directly —
///     `ErrorKind::Internal` in [`iherb_cli::app::json_document`],
///     `ErrorKind::Interrupted` on the interrupt path — or constructs an
///     [`IherbError`] variant whose real `kind()` is that kind. The mapping is
///     taken from the production function, not from a table here.
///
/// `src/error.rs` is excluded on purpose: `error_type`, `exit_code`, `ALL` and
/// `kind` each name every variant, so counting a mention there would make the
/// taxonomy its own witness — which is the circularity this test exists to
/// break. Comments are stripped for the same reason; a `[`IherbError::Navigation`]`
/// in a doc comment is a reference, not a producer.
///
/// # What it still does not prove
///
/// That a *run* can reach each code. This is a source-level check: it says the
/// crate constructs the error, not that any input makes it do so. The
/// behavioural tests below are what carry that half — `every_way_to_pass_bad_input_reports_invalid_input`,
/// `an_empty_result_set_is_the_catalog_end_not_an_internal_fault`,
/// `a_navigation_timeout_is_classified_from_the_type_not_the_message`,
/// `a_payload_that_will_not_serialize_fails_the_run`,
/// `an_interrupted_run_still_answers_in_json` — each through a real production
/// constructor. Between the two, a new code cannot be documented, listed and
/// shipped with nothing behind it.
#[test]
fn the_taxonomy_carries_no_code_without_a_producer() {
    let source = production_source_outside_the_taxonomy();

    // 1. `ALL` is the whole enum. Everything below sweeps it.
    let declared_kinds = declared_variants("ErrorKind");
    let listed: BTreeSet<String> = ErrorKind::ALL.iter().map(variant_name).collect();
    assert_eq!(
        declared_kinds, listed,
        "ErrorKind::ALL has fallen behind the enum; a variant missing from it is \
         exempt from every sweep in this file"
    );

    // 2. Every declared `IherbError` variant has a sample to classify.
    let samples = error_samples();
    let sampled: BTreeSet<String> = samples.iter().map(|(name, _)| name.to_string()).collect();
    assert_eq!(
        declared_variants("IherbError"),
        sampled,
        "error_samples() has fallen behind IherbError; a variant with no sample \
         here cannot be shown to have a producer"
    );

    // 3. What production actually constructs.
    let mut produced: BTreeSet<String> = BTreeSet::new();
    for (name, error) in &samples {
        if constructs(&source, "IherbError", name) {
            // The mapping comes from the production function. A variant whose
            // `kind()` is changed re-points its producer here, rather than this
            // test holding a second opinion about the taxonomy.
            produced.insert(variant_name(&error.kind()));
        }
    }
    for kind in ErrorKind::ALL {
        let name = variant_name(kind);
        if constructs(&source, "ErrorKind", &name) {
            produced.insert(name);
        }
    }

    let orphans: Vec<&String> = listed.difference(&produced).collect();
    assert!(
        orphans.is_empty(),
        "these codes are documented and nothing in this crate produces them: {:?}. \
         Wire a producer or remove the code — a caller branches on a number that \
         never arrives and never finds out.",
        orphans
    );

    // The four that were removed rather than wired, by name, so re-adding one
    // is a decision someone has to make in the open rather than a rebase
    // bringing back a row.
    for retired in ["network_error", "io_error", "cache_error", "json_error"] {
        assert!(
            !listed.contains(&pascal_case(retired)),
            "{} is back in the taxonomy; it needs a producer and a test that reaches it",
            retired
        );
    }
}

/// A variant's identifier as it is spelled in source.
///
/// Both enums are `Debug` and every variant swept here is a unit variant, so
/// the derived `Debug` *is* the identifier — no second hand-written table to
/// fall behind the enum.
fn variant_name(kind: &ErrorKind) -> String {
    format!("{:?}", kind)
}

/// `network_error` → `NetworkError`, so a retired `error_type` can be looked
/// for among variant identifiers.
fn pascal_case(snake: &str) -> String {
    snake
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// The variant identifiers an enum in `src/error.rs` declares.
///
/// Read out of the file rather than mirrored in an array here, for the same
/// reason [`documented_table`] reads the README: a copy is a thing that can
/// disagree, and the disagreement is invisible.
fn declared_variants(enum_name: &str) -> BTreeSet<String> {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/error.rs"))
        .expect("src/error.rs is the taxonomy");

    let body = text
        .split(&format!("pub enum {} {{\n", enum_name))
        .nth(1)
        .unwrap_or_else(|| panic!("src/error.rs no longer declares `pub enum {}`", enum_name));
    // The first closing brace in column zero ends the declaration; a struct
    // variant's own brace is indented.
    let body = body.split("\n}").next().unwrap();

    let variants: BTreeSet<String> = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//") && !line.starts_with('#'))
        .filter_map(|line| {
            let ident: String = line
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            let rest = line[ident.len()..].trim_start();
            // A variant is an identifier that starts a line and is followed by
            // its payload or a comma. A struct variant's fields are lower case
            // and its closing `},` has no identifier at all.
            let starts_upper = ident.chars().next().is_some_and(|c| c.is_ascii_uppercase());
            let is_variant = matches!(rest.chars().next(), Some('(' | '{' | ',') | None);
            (starts_upper && is_variant).then_some(ident)
        })
        .collect();

    assert!(
        variants.len() > 5,
        "`pub enum {}` did not parse; it yielded {:?}",
        enum_name,
        variants
    );
    variants
}

/// One value of every [`IherbError`] variant, so each can be handed to the
/// production `kind()`.
///
/// Checked against the declaration rather than trusted: a variant added without
/// a line here fails [`the_taxonomy_carries_no_code_without_a_producer`].
fn error_samples() -> Vec<(&'static str, IherbError)> {
    vec![
        ("InvalidInput", IherbError::InvalidInput("x".into())),
        ("BrowserLaunch", IherbError::BrowserLaunch("x".into())),
        ("Navigation", IherbError::Navigation("x".into())),
        (
            "NavigationTimeout",
            IherbError::NavigationTimeout("x".into()),
        ),
        ("CloudflareBlocked", IherbError::CloudflareBlocked(3)),
        ("ProductNotFound", IherbError::ProductNotFound("x".into())),
        ("ParseFailed", IherbError::ParseFailed("x".into())),
        (
            "EmptyPageOrCatalogEnd",
            IherbError::EmptyPageOrCatalogEnd("x".into()),
        ),
        (
            "CurrencyMismatch",
            IherbError::CurrencyMismatch {
                expected: "USD".into(),
                actual: "NOK".into(),
                what: "x".into(),
            },
        ),
        ("ChromeDownload", IherbError::ChromeDownload("x".into())),
    ]
}

/// Every `.rs` file the crate ships except `src/error.rs`, with comments
/// stripped.
fn production_source_outside_the_taxonomy() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let taxonomy = root.join("error.rs");
    let mut out = String::new();
    collect_rust_sources(&root, &taxonomy, &mut out);
    assert!(
        out.len() > 10_000,
        "the crate's source did not load; got {} bytes",
        out.len()
    );
    out
}

fn collect_rust_sources(dir: &Path, skip: &Path, out: &mut String) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {}", dir.display(), e))
        .map(|e| e.expect("readable directory entry").path())
        .collect();
    entries.sort();

    for path in entries {
        if path.is_dir() {
            collect_rust_sources(&path, skip, out);
        } else if path.extension().is_some_and(|e| e == "rs") && path != skip {
            let text =
                std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{:?}: {}", path, e));
            for line in text.lines() {
                // A reference in a doc comment is not a producer. Whole-line
                // comments are all this crate has; there are no block comments
                // and no trailing `//` after code that names either enum.
                if !line.trim_start().starts_with("//") {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
    }
}

/// Whether production code names `Ty::Variant`.
///
/// Boundary-checked, so `IherbError::Navigation` is not satisfied by
/// `IherbError::NavigationTimeout` — the two are different codes with opposite
/// advice, and a prefix match would let either stand in for the other.
fn constructs(source: &str, ty: &str, variant: &str) -> bool {
    let needle = format!("{}::{}", ty, variant);
    source.match_indices(&needle).any(|(at, _)| {
        !source[at + needle.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// `parse_failed` means *we loaded the page and could not read it* — the one
/// signal in the table worth waking someone for. The fork this taxonomy is
/// ported from classified every unrecognised error as `parse_failed`, which
/// makes it fire on everything and therefore mean nothing.
#[test]
fn an_unrecognised_error_is_internal_and_never_parse_failed() {
    let anonymous = anyhow::anyhow!("something nobody typed");
    assert_eq!(classify_error(&anonymous), ErrorKind::Internal);
    assert_ne!(classify_error(&anonymous), ErrorKind::ParseFailed);
    assert_eq!(ErrorKind::Internal.exit_code(), 70);
}

/// The classification has to survive whatever the error is wrapped in, or it
/// never fires in production: the `IherbError` is almost never the outermost
/// error.
///
/// Two wrappings, and the second is the one that earns the `.chain()` walk.
/// `anyhow` recurses through its own `.context(..)` layers on a bare
/// `downcast_ref`, so a test built only out of contexts passes whether the
/// classifier walks the chain or looks only at the top — it is a test that
/// cannot fail, which is worse than no test. An error linked by
/// `Error::source` is not `anyhow`'s to recurse through, and only the walk
/// finds it.
#[test]
fn classification_finds_the_error_however_it_is_wrapped() {
    use anyhow::Context;

    let contexts: anyhow::Result<()> = Err(IherbError::CloudflareBlocked(3).into());
    let contexts = contexts
        .context("Failed to navigate to the product page")
        .context("while fetching product 12949")
        .unwrap_err();

    assert_eq!(classify_error(&contexts), ErrorKind::CloudflareBlocked);
    assert_eq!(classify_error(&contexts).exit_code(), 22);

    let sourced = anyhow::Error::new(Wrapping(IherbError::ProductNotFound("12949".into())));
    assert!(
        sourced.downcast_ref::<IherbError>().is_none(),
        "the point of this half is that the top of the error is not the IherbError"
    );
    assert_eq!(classify_error(&sourced), ErrorKind::ProductNotFound);
}

/// An error that carries an [`IherbError`] as its `source` rather than as
/// itself. Stands in for any third-party error type the pipeline may one day
/// wrap ours in.
#[derive(Debug)]
struct Wrapping(IherbError);

impl std::fmt::Display for Wrapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "something went wrong while working")
    }
}

impl std::error::Error for Wrapping {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// Every `IherbError` variant lands on its own square.
#[test]
fn each_error_variant_maps_to_its_own_kind() {
    let cases: Vec<(IherbError, ErrorKind)> = vec![
        (
            IherbError::InvalidInput("nope".into()),
            ErrorKind::InvalidInput,
        ),
        (
            IherbError::BrowserLaunch("nope".into()),
            ErrorKind::BrowserLaunchFailed,
        ),
        (
            IherbError::ChromeDownload("nope".into()),
            ErrorKind::ChromeDownloadFailed,
        ),
        (
            IherbError::NavigationTimeout("Failed to navigate to https://x".into()),
            ErrorKind::NavigationTimeout,
        ),
        // The message names a timeout and the kind is still `navigation_failed`,
        // which is the fix: the classification is the variant, not the prose.
        (
            IherbError::Navigation(
                "Failed to navigate to https://no.iherb.com/search?kw=timeout: closed".into(),
            ),
            ErrorKind::NavigationFailed,
        ),
        (
            IherbError::CloudflareBlocked(3),
            ErrorKind::CloudflareBlocked,
        ),
        (
            IherbError::ProductNotFound("12949".into()),
            ErrorKind::ProductNotFound,
        ),
        (
            IherbError::EmptyPageOrCatalogEnd("nothing".into()),
            ErrorKind::EmptyPageOrCatalogEnd,
        ),
        (
            IherbError::ParseFailed("12949".into()),
            ErrorKind::ParseFailed,
        ),
        // Its own code, not `invalid_input`. The command line was well formed;
        // this is the storefront's answer disagreeing with it, and the two call
        // for different repairs.
        (
            IherbError::CurrencyMismatch {
                expected: "CHF".into(),
                actual: "USD".into(),
                what: "product 12949".into(),
            },
            ErrorKind::CurrencyMismatch,
        ),
    ];

    for (error, expected) in cases {
        let rendered = error.to_string();
        assert_eq!(
            classify_error(&anyhow::Error::new(error)),
            expected,
            "{}",
            rendered
        );
    }
}

// ---------------------------------------------------------------------------
// The codes have producers. This is the half the issue's table was missing.
// ---------------------------------------------------------------------------

fn config(country: &str, currency: Option<&str>) -> AppConfig {
    AppConfig::load(
        Some(country.to_string()),
        currency.map(str::to_string),
        false,
        None,
        false,
    )
    .expect("config with a known country")
}

/// Each of these was an untyped `anyhow::bail!` (or, for the country code, an
/// `IherbError::Navigation`) and so classified as `internal_error` — "this tool
/// is broken, file a bug" — for input a caller could simply correct. The
/// country code was worse: `navigation_failed` tells a caller to retry a
/// network problem, on an argument that will never become valid.
///
/// Asserted one site at a time rather than as a class, so reverting any single
/// one of them shows up as its own failure.
#[test]
fn every_way_to_pass_bad_input_reports_invalid_input() {
    let config = config("us", None);

    let empty_query = SearchTarget::new_err(&config, "", 20, SortOrder::Relevance, None);
    assert_eq!(classify_error(&empty_query), ErrorKind::InvalidInput);

    let zero_limit = SearchTarget::new_err(&config, "vitamin c", 0, SortOrder::Relevance, None);
    assert_eq!(classify_error(&zero_limit), ErrorKind::InvalidInput);

    let unknown_category = iherb_cli::scraper::search::CategoryId::resolve("not-a-category")
        .expect_err("an unknown category name is an error");
    assert_eq!(classify_error(&unknown_category), ErrorKind::InvalidInput);

    let bad_identifier = iherb_cli::targets::ProductTarget::new(&config, "not-an-id")
        .err()
        .expect("a non-numeric, non-URL identifier is an error");
    assert_eq!(classify_error(&bad_identifier), ErrorKind::InvalidInput);

    let unknown_country = AppConfig::validate_country("zz")
        .expect_err("an unsupported subdomain is an error")
        .into();
    assert_eq!(classify_error(&unknown_country), ErrorKind::InvalidInput);

    assert_eq!(ErrorKind::InvalidInput.exit_code(), 2);
}

/// An empty search is the most ordinary failure this tool has, and as an
/// untyped error it classified as `internal_error` (70) — the code that means
/// the tool itself is broken.
///
/// It is still an *error*, and whether it should be is a live question that
/// belongs with the batch and catalog work (#10, #21): a caller walking a
/// catalog would rather have an empty list and a zero exit. This only pins the
/// name the existing behaviour now goes by.
#[test]
fn an_empty_result_set_is_the_catalog_end_not_an_internal_fault() {
    let config = config("us", None);
    let target = SearchTarget::new(&config, "vitamin c", 20, SortOrder::Relevance, None)
        .expect("a valid search");

    let empty = SearchResult {
        query: "vitamin c".to_string(),
        total_results: None,
        products: Vec::new(),
        fetch: SearchFetch {
            pages_fetched: Some(1),
            exhausted: Some(true),
        },
    };

    let error = target
        .validate(&empty)
        .expect_err("a result set with no products does not validate");
    assert_eq!(classify_error(&error), ErrorKind::EmptyPageOrCatalogEnd);
    assert_ne!(classify_error(&error), ErrorKind::Internal);
    assert_eq!(ErrorKind::EmptyPageOrCatalogEnd.exit_code(), 24);
}

// A shim so the assertions above read as one line each. `SearchTarget::new`
// returns a target we do not want, and only ever its error.
trait SearchTargetErr {
    fn new_err(
        config: &AppConfig,
        query: &str,
        limit: usize,
        sort: SortOrder,
        category: Option<&str>,
    ) -> anyhow::Error;
}

impl SearchTargetErr for SearchTarget {
    fn new_err(
        config: &AppConfig,
        query: &str,
        limit: usize,
        sort: SortOrder,
        category: Option<&str>,
    ) -> anyhow::Error {
        SearchTarget::new(config, query, limit, sort, category)
            .err()
            .expect("this target is not constructible")
    }
}

use iherb_cli::targets::SearchTarget;

// ---------------------------------------------------------------------------
// The envelope (#44)
// ---------------------------------------------------------------------------

fn at(epoch_secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(epoch_secs)
}

/// `fetched_at` is when the page was read and `emitted_at` is when the command
/// ran. The distinction is the entire point when `from_cache` is true: a record
/// stored in a spreadsheet and read back months later otherwise carries one
/// timestamp with two possible meanings.
///
/// Built the way the pipeline builds it — [`Fetched::cached`] is what
/// `crate::fetch::cached` returns on a hit — rather than by handing [`Meta`] a
/// `Provenance` this test invented.
#[test]
fn a_cached_document_dates_the_page_earlier_than_the_run() {
    let config = config("no", Some("NOK"));
    let emitted = at(1_756_000_000);
    let written = at(1_756_000_000 - 3 * 24 * 60 * 60);

    let fetched = Fetched::cached("a record", written);
    let meta = Meta::new(&config, Some(fetched.provenance), emitted);

    assert_eq!(meta.from_cache, Some(true));
    assert_eq!(meta.fetched_at.as_deref(), Some("2025-08-21T01:46:40Z"));
    assert_eq!(meta.emitted_at, "2025-08-24T01:46:40Z");
    assert!(meta.fetched_at.as_deref().unwrap() < meta.emitted_at.as_str());
}

/// A fresh document dates the page and the run alike — #44's criterion, and the
/// README's "the same instant".
///
/// # Why this goes through `Fetched::fresh`
///
/// Because the previous version of this test could not fail. It built a
/// `Provenance` itself, put the same `now` in both fields, and asserted the two
/// came back equal — so changing the production path's fresh timestamp to
/// `UNIX_EPOCH` left it green. It was asserting on its own fixture.
///
/// [`Fetched::fresh`] is the one constructor the pipeline uses for a fresh
/// result, and [`Provenance::Fresh`] carries no instant at all: a fresh record's
/// page was read during this run, so the run's single clock sample dates both
/// fields and there is no second one to drift. `emitted` here is deliberately
/// not `SystemTime::now()`, so any clock read on the fresh path shows up as a
/// mismatch rather than as a race this test would usually win.
#[test]
fn a_fresh_document_dates_the_page_and_the_run_alike() {
    let config = config("no", Some("NOK"));
    let emitted = at(1_756_000_000);

    let fetched = Fetched::fresh("a record");
    let meta = Meta::new(&config, Some(fetched.provenance), emitted);

    assert_eq!(meta.from_cache, Some(false));
    assert_eq!(meta.emitted_at, "2025-08-24T01:46:40Z");
    assert_eq!(
        meta.fetched_at.as_deref(),
        Some(meta.emitted_at.as_str()),
        "a fresh record's page was read during this run; the two have to be one instant"
    );
}

/// The same claim one layer down, on [`Provenance`] itself, so that the
/// equality is visible as a property of the type rather than of one call.
#[test]
fn a_fresh_provenance_has_no_second_clock_to_drift_from() {
    let emitted = at(1_756_000_000);

    assert_eq!(Provenance::Fresh.fetched_at(emitted), emitted);
    assert!(!Provenance::Fresh.from_cache());

    let written = at(1_700_000_000);
    assert_eq!(Provenance::Cached(written).fetched_at(emitted), written);
    assert!(Provenance::Cached(written).from_cache());
}

/// The timestamp format itself, against instants whose UTC rendering is not in
/// question. The date arithmetic here is hand-rolled — the crate carries no date
/// dependency — and a record's `fetched_at` is the one field a consumer cannot
/// re-derive, so an off-by-one leap year would be silent and permanent.
#[test]
fn timestamps_render_as_rfc_3339_in_utc() {
    use iherb_cli::output::format_rfc3339;

    assert_eq!(format_rfc3339(at(0)), "1970-01-01T00:00:00Z");
    assert_eq!(format_rfc3339(at(1_756_000_000)), "2025-08-24T01:46:40Z");
    // 29 February 2024: a leap day in a leap century-rule year.
    assert_eq!(format_rfc3339(at(1_709_209_925)), "2024-02-29T12:32:05Z");
    // 1 March 2100, which is *not* a leap year: 2100 % 100 == 0, 2100 % 400 != 0.
    assert_eq!(format_rfc3339(at(4_107_542_400)), "2100-03-01T00:00:00Z");
}

/// A failure read no page, and says so with `null` rather than dating itself as
/// if it were data.
#[test]
fn a_failure_dates_no_page_at_all() {
    let config = config("no", Some("NOK"));
    let meta = Meta::new(&config, None, at(1_756_000_000));

    assert_eq!(meta.fetched_at, None);
    assert_eq!(meta.from_cache, None);
    // The storefront is still known: the run resolved, it just did not succeed.
    assert_eq!(
        meta.requested_storefront.as_deref(),
        Some("https://no.iherb.com")
    );
}

/// A command line clap refused has no effective storefront, and inventing one
/// would be a claim about a run that never started.
#[test]
fn a_run_that_never_configured_claims_no_storefront() {
    let meta = Meta::unconfigured(None, at(1_756_000_000));
    assert_eq!(meta.requested_country, None);
    assert_eq!(meta.requested_currency, None);
    assert_eq!(meta.requested_storefront, None);
    assert_eq!(meta.tool_version, env!("CARGO_PKG_VERSION"));
}

/// The storefront in `meta` is the one the run resolved to, and this is the
/// case that has to be right for a non-US corpus: a US-shaped assumption would
/// hide exactly here.
#[test]
fn the_envelope_names_a_non_usd_storefront() {
    let config = config("no", Some("NOK"));
    let meta = Meta::new(&config, None, at(1_756_000_000));

    assert_eq!(meta.requested_country.as_deref(), Some("no"));
    assert_eq!(meta.requested_currency.as_deref(), Some("NOK"));
    assert_eq!(
        meta.requested_storefront.as_deref(),
        Some("https://no.iherb.com")
    );
}

/// `meta` describes the request, and its field names have to say so.
///
/// The case that made this a bug rather than a quibble: no flags at all, on a
/// Norwegian IP. The configuration resolves `us`, so `meta` claimed country
/// `us` and storefront `www.iherb.com` with `currency: null` — while iHerb,
/// which geolocates by IP (#5), can price the very record beside it in NOK. A
/// field named `country` sitting next to `data` reads as a fact about `data`.
/// Named `requested_country` it cannot.
#[test]
fn meta_names_the_request_and_never_claims_to_name_the_answer() {
    let config = config("us", None);
    let meta = Meta::new(&config, Some(Provenance::Fresh), at(1_756_000_000));
    let rendered: Value = serde_json::from_str(
        &Envelope::success(meta, serde_json::json!({"currency": "NOK"})).render(),
    )
    .expect("the envelope is JSON");

    let meta = &rendered["meta"];
    assert_eq!(meta["requested_country"], "us");
    assert_eq!(meta["requested_storefront"], "https://www.iherb.com");
    assert!(meta["requested_currency"].is_null());

    // No unprefixed alias survives, in either direction: a consumer must not be
    // able to read a request value as an answer, and the answer must not be
    // duplicated here where it could drift from the record.
    for claimed in ["country", "currency", "storefront"] {
        assert!(
            meta.get(claimed).is_none(),
            "meta.{} reads as a fact about data; it is a fact about the request",
            claimed
        );
    }

    // Where the answer actually lives, and it is the only place it lives.
    assert_eq!(rendered["data"]["currency"], "NOK");
}

/// `--currency` is a question the run asked the storefront, not an answer about
/// a price. A run that asked nothing says `null` here — and the currency a
/// price is actually in lives on the record, with its provenance (#5).
#[test]
fn asking_for_no_currency_claims_no_currency() {
    let config = config("no", None);
    let meta = Meta::new(&config, None, at(1_756_000_000));

    assert_eq!(meta.requested_currency, None);
    assert_eq!(
        meta.requested_storefront.as_deref(),
        Some("https://no.iherb.com")
    );
}

/// A payload that will not serialize produces a failure envelope **and a
/// failing exit code**.
///
/// It used to produce only the first. `render_json` built the `json_error`
/// envelope and `run` returned `ExitCode::SUCCESS` beside it, so the one
/// reachable path to that code reported `0` — a document saying `ok: false`
/// under an exit code saying it succeeded, which is the single combination a
/// caller branching on the code cannot recover from.
///
/// The kind is `internal_error`, not a `json_error` of its own: this tool
/// consumes no JSON from a caller, and a record of ours that will not serialize
/// is our bug.
#[test]
fn a_payload_that_will_not_serialize_fails_the_run() {
    use iherb_cli::app::json_document;

    let config = config("no", Some("NOK"));
    let meta = || Meta::new(&config, Some(Provenance::Fresh), at(1_756_000_000));

    let (document, code) = json_document(meta(), Ok(serde_json::json!({"answer": 42})));
    assert_eq!(code, 0);
    let ok: Value = serde_json::from_str(&document).expect("the envelope is JSON");
    assert_eq!(ok["ok"], true);

    let broken = serde_json::from_str::<Value>("{").unwrap_err();
    let (document, code) = json_document(meta(), Err(broken));
    assert_eq!(
        code,
        ErrorKind::Internal.exit_code(),
        "an envelope reporting a failure must not exit 0"
    );
    let failed: Value = serde_json::from_str(&document).expect("the envelope is JSON");
    assert_eq!(failed["ok"], false);
    assert_eq!(failed["error_type"], "internal_error");
}

/// A failure that happened *after* a page was read reports the page.
///
/// `parse_failed` and `currency_mismatch` both mean the same thing about
/// provenance: the browser launched, the page loaded, and the record it
/// produced was rejected. The envelope used to answer `fetched_at: null` and
/// `from_cache: null` for those, because provenance was assembled only once
/// `fetch` had succeeded — stating, of a page that was read, that none was.
#[test]
fn a_failure_after_a_page_was_read_says_a_page_was_read() {
    use iherb_cli::fetch::Failure;

    let config = config("no", Some("NOK"));
    let emitted = at(1_756_000_000);

    let failure = Failure::after_page_read(anyhow::Error::new(IherbError::ParseFailed(
        "143499".to_string(),
    )));
    let meta = Meta::new(&config, failure.provenance, emitted);

    assert_eq!(meta.from_cache, Some(false));
    assert_eq!(
        meta.fetched_at.as_deref(),
        Some(meta.emitted_at.as_str()),
        "the page was read during this run"
    );

    // And a failure that read nothing still says nothing.
    let never_started: Failure = anyhow::Error::new(IherbError::InvalidInput("no".into())).into();
    assert!(never_started.provenance.is_none());
    let meta = Meta::new(&config, never_started.provenance, emitted);
    assert_eq!(meta.fetched_at, None);
    assert_eq!(meta.from_cache, None);
}

/// The timeout distinction is made from `chromiumoxide`'s own typed error, at
/// the boundary, before anything is flattened into a string.
///
/// It used to be made from the string afterwards, by looking for "timeout" in
/// the message — and the message embeds the URL the run asked for. So
/// `iherb-cli search timeout` put the word into every navigation failure of
/// that run, and a caller was told to retry a failure that had nothing to do
/// with the clock. **User input steering a retry decision.** The heuristic was
/// equally fragile the other way: one wording change upstream and a real
/// timeout starts reporting 21.
#[test]
fn a_navigation_timeout_is_classified_from_the_type_not_the_message() {
    use chromiumoxide::error::CdpError;
    use iherb_cli::scraper::navigation::navigation_failure;

    let timed_out = navigation_failure(
        "Failed to navigate to https://no.iherb.com/pr/p/143499",
        CdpError::Timeout,
    );
    assert_eq!(timed_out.kind(), ErrorKind::NavigationTimeout);
    assert_eq!(timed_out.kind().exit_code(), 20);

    // The message says "timeout" — because the caller searched for it — and the
    // driver did not. It is not a timeout.
    let query_says_timeout = navigation_failure(
        "Failed to navigate to https://no.iherb.com/search?kw=timeout",
        CdpError::NoResponse,
    );
    assert_eq!(
        query_says_timeout.kind(),
        ErrorKind::NavigationFailed,
        "a caller's own query text must not be able to steer its retry decision"
    );
    assert!(
        query_says_timeout.to_string().contains("timeout"),
        "the message really does carry the word; that is the whole trap"
    );

    // And a failure with nothing timeout-shaped about it either way.
    let plain = navigation_failure("Failed to get page content", CdpError::NotFound);
    assert_eq!(plain.kind(), ErrorKind::NavigationFailed);
}

#[test]
fn success_and_failure_wear_the_same_envelope() {
    let config = config("no", Some("NOK"));
    let meta = || Meta::new(&config, None, at(1_756_000_000));

    let ok: Value = serde_json::from_str(
        &Envelope::success(meta(), serde_json::json!({"answer": 42})).render(),
    )
    .expect("the success envelope is JSON");
    let bad: Value = serde_json::from_str(
        &Envelope::failure(meta(), ErrorKind::CloudflareBlocked, "blocked".into()).render(),
    )
    .expect("the failure envelope is JSON");

    for document in [&ok, &bad] {
        assert_eq!(document["schema_version"], SCHEMA_VERSION);
        assert_eq!(
            document["meta"]["requested_storefront"],
            "https://no.iherb.com"
        );
    }

    assert_eq!(ok["ok"], true);
    assert_eq!(ok["data"]["answer"], 42);
    assert!(ok.get("error_type").is_none());

    assert_eq!(bad["ok"], false);
    assert_eq!(bad["error_type"], "cloudflare_blocked");
    assert_eq!(bad["message"], "blocked");
    assert!(bad.get("data").is_none());
}

// ---------------------------------------------------------------------------
// The payloads (#9, on #28's seam)
// ---------------------------------------------------------------------------

/// A product with the shapes the current corpus actually produces: a page that
/// publishes no supplement facts at all, and an availability signal nobody
/// found. Both are meaningful values in the JSON rather than omissions.
fn a_product() -> ProductDetail {
    let mut product = ProductDetail {
        name: "Biocidin Botanicals, Dentalcidin".to_string(),
        brand: "Biocidin Botanicals".to_string(),
        price: 289.0,
        original_price: None,
        currency: Some("NOK".to_string()),
        rating: Some(4.6),
        review_count: Some(212),
        product_url: "https://no.iherb.com/pr/143499".to_string(),
        product_id: "143499".to_string(),
        in_stock: None,
        description: Some("Toothpaste.".to_string()),
        product_code: Some("BIO-00143".to_string()),
        upc: Some("859293004006".to_string()),
        ingredients: Some("Water, glycerin.".to_string()),
        supplement_facts: None,
        suggested_use: Some("Brush.".to_string()),
        warnings: None,
        shipping_weight: Some("0.2 kg".to_string()),
        category_breadcrumb: Some(vec!["Bath".to_string()]),
        review_distribution: None,
        extraction: Extraction::new(Strategy::JsonLd),
    };
    product.claim_unattributed(Source::JsonLd);
    product
}

fn a_search_result() -> SearchResult {
    let mut card = ProductSummary {
        name: "Nordic Naturals, Ultimate Omega".to_string(),
        brand: "Nordic Naturals".to_string(),
        price: Some(880.63),
        original_price: None,
        currency: Some("NOK".to_string()),
        rating: Some(4.8),
        review_count: Some(24_938),
        product_url: "https://no.iherb.com/pr/12949".to_string(),
        product_id: "12949".to_string(),
        in_stock: None,
        extraction: Extraction::new(Strategy::Dom),
    };
    card.claim_unattributed(Source::Dom);

    SearchResult {
        query: "omega 3".to_string(),
        total_results: Some(1200),
        products: vec![card],
        fetch: SearchFetch {
            pages_fetched: Some(1),
            exhausted: Some(false),
        },
    }
}

/// #28's seam, pinned. The `extraction` block is
/// `serde_json::to_value(product.health())` and nothing else: no added fields,
/// nothing computed, nothing flattened or renamed.
#[test]
fn the_extraction_block_is_health_verbatim() {
    let product = a_product();
    let document = format_product_json(&product, &ProductView::everything()).unwrap();

    assert_eq!(
        document["extraction"],
        serde_json::to_value(product.health()).unwrap()
    );

    // Not the raw `Extraction` the record serializes by default, which carries
    // the source map and none of the derived lists.
    let raw = serde_json::to_value(&product).unwrap();
    assert_ne!(document["extraction"], raw["extraction"]);
    assert!(document["extraction"]["fields_absent"].is_array());
    assert!(document["extraction"]["fields_malformed"].is_array());
    assert!(document["extraction"]["degraded"].is_boolean());
}

/// One provenance shape across both commands (#49). A search that rendered
/// cards with no `extraction` block, or with the raw one, would leave a caller
/// unable to ask the same question of the two commands.
#[test]
fn search_cards_carry_the_same_extraction_block_products_do() {
    let result = a_search_result();
    let document = format_search_json(&result).unwrap();

    let card = &document["products"][0];
    assert_eq!(
        card["extraction"],
        serde_json::to_value(result.products[0].health()).unwrap()
    );

    let product = format_product_json(&a_product(), &ProductView::everything()).unwrap();
    let keys = |v: &Value| {
        let mut k: Vec<String> = v["extraction"]
            .as_object()
            .expect("extraction is an object")
            .keys()
            .cloned()
            .collect();
        k.sort();
        k
    };
    assert_eq!(keys(card), keys(&product));

    // The walk's own report survives too: a short result is only interpretable
    // beside whether iHerb ran out (#6).
    assert_eq!(document["fetch"]["exhausted"], false);
}

/// `null` is a value here, not an omission. `in_stock` is `Option<bool>`
/// because "no signal on the page said either way" is a different answer from
/// "no" (#31, #49), and a page with no Supplement Facts panel — product 143499
/// in the current corpus — really has none.
#[test]
fn absent_values_are_rendered_as_null_rather_than_dropped() {
    let document = format_product_json(&a_product(), &ProductView::everything()).unwrap();

    assert!(document.get("in_stock").is_some());
    assert_eq!(document["in_stock"], Value::Null);
    assert!(document.get("supplement_facts").is_some());
    assert_eq!(document["supplement_facts"], Value::Null);
    assert_eq!(document["warnings"], Value::Null);

    let search = format_search_json(&a_search_result()).unwrap();
    assert_eq!(search["products"][0]["in_stock"], Value::Null);
}

/// `--section` is honoured under `--json` rather than silently ignored, and it
/// can never take provenance away with it.
#[test]
fn a_section_narrows_the_document_but_never_drops_provenance() {
    let product = a_product();

    let nutrition = format_product_json(
        &product,
        &ProductView::for_section(Some(Section::Nutrition)),
    )
    .unwrap();
    let mut keys: Vec<&String> = nutrition.as_object().unwrap().keys().collect();
    keys.sort();
    let mut expected: Vec<String> = ALWAYS_RENDERED.iter().map(|k| k.to_string()).collect();
    expected.push("supplement_facts".to_string());
    expected.sort();
    assert_eq!(
        keys,
        expected.iter().collect::<Vec<_>>(),
        "--section nutrition should carry the facts, the record's identity and its provenance"
    );
    assert_eq!(
        nutrition["extraction"],
        serde_json::to_value(product.health()).unwrap()
    );

    // The one section that means two: supplement facts *are* the active
    // ingredients, decided once in `ProductView` so both renderings agree.
    let ingredients = format_product_json(
        &product,
        &ProductView::for_section(Some(Section::Ingredients)),
    )
    .unwrap();
    assert!(ingredients.get("ingredients").is_some());
    assert!(
        ingredients.get("supplement_facts").is_some(),
        "--section ingredients means both lists, in JSON as in Markdown"
    );
    assert!(ingredients.get("price").is_none());
}

/// Every field of the model is placed in a section, so a projection cannot
/// silently drop one — and adding a field to `ProductDetail` without deciding
/// where it belongs is a failure here rather than a field that disappears the
/// moment anyone passes `--section`.
#[test]
fn every_field_of_the_record_belongs_to_some_section() {
    let mut rendered = product_json_keys(&a_product()).unwrap();
    rendered.sort();

    let accounted = accounted_json_keys();
    let mut accounted: Vec<String> = accounted.iter().map(|k| k.to_string()).collect();
    accounted.sort();

    assert_eq!(rendered, accounted);
}

// ---------------------------------------------------------------------------
// argv sniffing (#9): clap fails before the struct exists
// ---------------------------------------------------------------------------

#[test]
fn the_json_flag_is_found_before_clap_parses() {
    assert!(wants_json(["iherb-cli", "product", "12949", "--json"]));
    assert!(wants_json(["iherb-cli", "--json", "search", "zinc"]));
    assert!(!wants_json(["iherb-cli", "product", "12949"]));

    // After `--` every word is a value, so a query that happens to read
    // `--json` is not a request for JSON.
    assert!(!wants_json(["iherb-cli", "search", "--", "--json"]));
}

// ---------------------------------------------------------------------------
// The process: exactly one document on stdout, and nothing else
// ---------------------------------------------------------------------------

/// A throwaway `$HOME`, so the child process reads a config file and a cache
/// this test wrote rather than the developer's own.
struct Home(PathBuf);

impl Home {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "iherb-cli-json-{}-{}-{}",
            label,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create temp home");
        Self(path)
    }

    #[cfg(target_os = "macos")]
    fn config_dir(&self) -> PathBuf {
        self.0.join("Library/Application Support/iherb-cli")
    }

    #[cfg(not(target_os = "macos"))]
    fn config_dir(&self) -> PathBuf {
        self.0.join(".config/iherb-cli")
    }

    #[cfg(target_os = "macos")]
    fn cache_dir(&self) -> PathBuf {
        self.0.join("Library/Caches/iherb-cli")
    }

    #[cfg(not(target_os = "macos"))]
    fn cache_dir(&self) -> PathBuf {
        self.0.join(".cache/iherb-cli")
    }

    fn write_config(&self, toml: &str) {
        let dir = self.config_dir();
        std::fs::create_dir_all(&dir).expect("create config dir");
        std::fs::write(dir.join("config.toml"), toml).expect("write config file");
    }

    /// Seed a cache entry, so the run below answers from disk and never starts
    /// Chrome.
    fn seed_product(&self, key: &CacheKey, product: &ProductDetail) {
        let dir = self.cache_dir();
        std::fs::create_dir_all(&dir).expect("create cache dir");
        std::fs::write(
            dir.join(key.file_name()),
            serde_json::to_string_pretty(product).expect("serialize the seeded product"),
        )
        .expect("write cache entry");
    }

    fn run(&self, args: &[&str], log: Option<&str>) -> Ran {
        let mut command = Command::new(env!("CARGO_BIN_EXE_iherb-cli"));
        command
            .args(args)
            .env("HOME", &self.0)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_CACHE_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("IHERB_COUNTRY")
            .env_remove("IHERB_CURRENCY")
            .env_remove("IHERB_BROWSER_PATH")
            .env_remove("RUST_LOG");
        if let Some(log) = log {
            command.env("RUST_LOG", log);
        }

        let output = command.output().expect("run iherb-cli");
        Ran {
            code: output.status.code().expect("the process was not signalled"),
            stdout: String::from_utf8(output.stdout).expect("stdout is UTF-8"),
            stderr: String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Ran {
    /// The one document on stdout — and the assertion that it *is* one
    /// document, with nothing before or after it.
    fn document(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "stdout is not exactly one JSON document ({e}); it was:\n{}",
                self.stdout
            )
        })
    }
}

fn seeded_key() -> CacheKey {
    CacheKey::Product {
        country: "no".to_string(),
        currency: Some("NOK".to_string()),
        product_id: "143499".to_string(),
    }
}

/// The whole promise of `--json`, and the thing that used to break it: the
/// tracing subscriber wrote to **stdout**, so a log line landed in the middle
/// of the payload. It goes to stderr now, unconditionally — a warning has never
/// belonged on stdout.
///
/// `RUST_LOG` is turned up so there is something to misplace; at the default
/// level the cache-hit line never fires and this test would pass with the bug
/// still in.
#[test]
fn stdout_carries_the_payload_and_nothing_else() {
    let home = Home::new("stdout-only");
    home.seed_product(&seeded_key(), &a_product());

    let ran = home.run(
        &[
            "product",
            "143499",
            "--country",
            "no",
            "--currency",
            "NOK",
            "--json",
        ],
        Some("iherb_cli=debug"),
    );

    assert_eq!(ran.code, 0, "stderr was:\n{}", ran.stderr);
    let document = ran.document();
    assert_eq!(document["ok"], true);

    assert!(
        ran.stderr.contains("Cache hit"),
        "the log line has to exist somewhere, or this test proves nothing; stderr was:\n{}",
        ran.stderr
    );
    assert!(
        !ran.stdout.contains("Cache hit"),
        "a log line reached stdout:\n{}",
        ran.stdout
    );
}

/// The same fix, on the path that has always been wrong: Markdown output had
/// log lines interleaved with it too.
#[test]
fn markdown_output_keeps_its_logging_off_stdout() {
    let home = Home::new("markdown-stdout");
    home.seed_product(&seeded_key(), &a_product());

    let ran = home.run(
        &["product", "143499", "--country", "no", "--currency", "NOK"],
        Some("iherb_cli=debug"),
    );

    assert_eq!(ran.code, 0, "stderr was:\n{}", ran.stderr);
    assert!(ran.stdout.starts_with('#'), "stdout was:\n{}", ran.stdout);
    assert!(!ran.stdout.contains("Cache hit"));
    assert!(ran.stderr.contains("Cache hit"));
}

/// `meta.requested_country` is the value the run *resolved* to, not the flag it was
/// passed — asserted with a config file and no flags at all, because that is
/// the case a record produced by an unattended run is written under.
#[test]
fn the_envelope_reports_the_resolved_storefront_with_no_flags_passed() {
    let home = Home::new("config-file");
    home.write_config("[defaults]\ncountry = \"no\"\ncurrency = \"NOK\"\n");
    home.seed_product(&seeded_key(), &a_product());

    let ran = home.run(&["product", "143499", "--json"], None);

    assert_eq!(ran.code, 0, "stderr was:\n{}", ran.stderr);
    let meta = &ran.document()["meta"];
    assert_eq!(meta["requested_country"], "no");
    assert_eq!(meta["requested_currency"], "NOK");
    assert_eq!(meta["requested_storefront"], "https://no.iherb.com");
    assert_eq!(meta["tool_version"], env!("CARGO_PKG_VERSION"));
}

/// A cache hit says so, and dates the page earlier than the run.
#[test]
fn a_cache_hit_says_it_is_a_cache_hit() {
    let home = Home::new("from-cache");
    home.seed_product(&seeded_key(), &a_product());

    let ran = home.run(
        &[
            "product",
            "143499",
            "--country",
            "no",
            "--currency",
            "NOK",
            "--json",
        ],
        None,
    );

    assert_eq!(ran.code, 0, "stderr was:\n{}", ran.stderr);
    let document = ran.document();
    assert_eq!(document["meta"]["from_cache"], true);
    assert_eq!(document["schema_version"], SCHEMA_VERSION);

    let fetched = document["meta"]["fetched_at"].as_str().unwrap();
    let emitted = document["meta"]["emitted_at"].as_str().unwrap();
    assert!(fetched <= emitted, "{} is after {}", fetched, emitted);

    // And the payload is the record, with #28's provenance block on it.
    assert_eq!(document["data"]["product_id"], "143499");
    assert_eq!(document["data"]["currency"], "NOK");
    assert_eq!(
        document["data"]["extraction"],
        serde_json::to_value(a_product().health()).unwrap()
    );
}

/// A failure is one JSON document too, with a stable `error_type` beside the
/// same envelope, and the exit code that goes with it.
#[test]
fn a_failing_run_answers_in_json_and_exits_on_its_own_code() {
    let home = Home::new("failure");

    let ran = home.run(&["search", "", "--json"], None);
    assert_eq!(ran.code, i32::from(ErrorKind::InvalidInput.exit_code()));

    let document = ran.document();
    assert_eq!(document["ok"], false);
    assert_eq!(document["error_type"], "invalid_input");
    assert_eq!(document["schema_version"], SCHEMA_VERSION);
    assert!(document["message"].as_str().unwrap().contains("empty"));
    assert!(document["meta"]["fetched_at"].is_null());
    assert!(document["meta"]["from_cache"].is_null());
    assert!(ran.stdout.starts_with('{'), "stdout was:\n{}", ran.stdout);
}

/// Clap fails before the parsed struct exists, so `--json` has to be found in
/// `argv`. Without that, a command line with a typo in it answers a machine in
/// prose.
#[test]
fn a_command_line_clap_refuses_still_answers_in_json() {
    let home = Home::new("clap");

    let ran = home.run(&["--json", "produkt", "143499"], None);
    assert_eq!(ran.code, i32::from(ErrorKind::InvalidInput.exit_code()));

    let document = ran.document();
    assert_eq!(document["ok"], false);
    assert_eq!(document["error_type"], "invalid_input");
    // clap's own text, unstyled: an ANSI escape inside a JSON string is a
    // message no consumer can read.
    let message = document["message"].as_str().unwrap();
    assert!(message.contains("produkt"), "{}", message);
    assert!(!message.contains('\u{1b}'), "{}", message);

    // The same command line without `--json` keeps clap's own behaviour.
    let plain = home.run(&["produkt", "143499"], None);
    assert_eq!(plain.code, 2);
    assert!(plain.stdout.is_empty());
    assert!(plain.stderr.contains("produkt"));
}

/// `--help` and `--version` are not command output and are not errors. Wrapping
/// a usage message in an envelope gives a machine a JSON document full of help
/// text and gives a human a worse one.
#[test]
fn help_is_still_help_under_json() {
    let home = Home::new("help");

    let helped = home.run(&["--json", "--help"], None);
    assert_eq!(helped.code, 0);
    assert!(helped.stdout.contains("--json"), "{}", helped.stdout);

    let versioned = home.run(&["--json", "--version"], None);
    assert_eq!(versioned.code, 0);
    assert!(versioned.stdout.contains(env!("CARGO_PKG_VERSION")));
}

/// **An interrupted run still emits one JSON document.**
///
/// This is the promise `--json` makes — one document on stdout, always, success
/// or failure — and the interrupt was the one path that kept none of it: exit
/// 130 with zero bytes written, measured. An agent that gives up on a slow fetch
/// got nothing to parse in the case where "always" matters most.
///
/// Run against a stand-in browser that never speaks, so the interrupt lands
/// while the run is genuinely inside the command — no network, no Chrome, and
/// the real signal handler, the real shutdown and the real render. Nothing about
/// the interrupt handling itself changed to make this pass; a document was added
/// to a path that already exited cleanly (#46).
#[cfg(unix)]
#[test]
fn an_interrupted_run_still_answers_in_json() {
    use std::io::Write;
    use std::process::Stdio;

    let home = Home::new("interrupt");

    // Executable, and it outlives the interrupt without ever printing the
    // DevTools line chromiumoxide waits for — so the launch is still in flight
    // when the signal lands. Its stderr stays attached on purpose: that is the
    // pipe chromiumoxide reads the line from, and closing it would end the
    // launch immediately with an error instead of leaving it waiting. It never
    // holds this test's own pipes, which chromiumoxide gives it as `null`.
    let stub = home.0.join("a-browser-that-never-speaks");
    let mut script = std::fs::File::create(&stub).expect("create the stub browser");
    script
        .write_all(b"#!/bin/sh\nexec sleep 20 </dev/null\n")
        .expect("write the stub browser");
    drop(script);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
            .expect("make the stub browser executable");
    }

    let child = Command::new(env!("CARGO_BIN_EXE_iherb-cli"))
        .args(["product", "143499", "--json", "--no-cache"])
        .env("HOME", &home.0)
        .env("IHERB_BROWSER_PATH", &stub)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_CACHE_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("IHERB_COUNTRY")
        .env_remove("IHERB_CURRENCY")
        .env_remove("RUST_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run iherb-cli");

    // Long enough for the handler to be installed and the launch to be in
    // flight; far short of the stub's lifetime.
    std::thread::sleep(Duration::from_millis(1500));
    let killed = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("send SIGINT");
    assert!(killed.success(), "could not interrupt the run");

    let output = child
        .wait_with_output()
        .expect("collect the interrupted run");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    assert_eq!(
        output.status.code(),
        Some(i32::from(ErrorKind::Interrupted.exit_code())),
        "stdout was:\n{}\nstderr was:\n{}",
        stdout,
        stderr
    );

    let document: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "an interrupted --json run wrote no document a caller can parse ({e}); \
             stdout was:\n{stdout}\nstderr was:\n{stderr}"
        )
    });
    assert_eq!(document["ok"], false);
    assert_eq!(document["error_type"], "interrupted");
    assert_eq!(document["schema_version"], SCHEMA_VERSION);
    // The run resolved its configuration before it was interrupted, so the
    // envelope still says what it had asked for.
    assert_eq!(document["meta"]["requested_country"], "us");
    // And nothing was read, which is a different claim from "read nothing".
    assert!(document["meta"]["fetched_at"].is_null());
    assert!(document["meta"]["from_cache"].is_null());
}

/// The seeded entry is where the run actually reads from — otherwise every
/// process test above would be launching a browser and this file would be a
/// network test wearing a disguise.
#[test]
fn the_seeded_cache_entry_is_the_one_the_run_reads() {
    let home = Home::new("seed-path");
    home.seed_product(&seeded_key(), &a_product());

    let seeded: PathBuf = home.cache_dir().join(seeded_key().file_name());
    assert!(Path::new(&seeded).exists());
    assert!(seeded.to_string_lossy().contains("_no_NOK_143499"));
}
