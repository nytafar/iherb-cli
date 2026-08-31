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

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use iherb_cli::app::wants_json;
use iherb_cli::cache::CacheKey;
use iherb_cli::cli::{Section, SortOrder};
use iherb_cli::config::AppConfig;
use iherb_cli::error::{classify_error, ErrorKind, IherbError};
use iherb_cli::fetch::FetchTarget;
use iherb_cli::model::{
    Extraction, ProductDetail, ProductSummary, SearchFetch, SearchResult, Source, Strategy,
};
use iherb_cli::output::{
    accounted_json_keys, format_product_json, format_search_json, product_json_keys, Envelope,
    Meta, ProductView, Provenance, ALWAYS_RENDERED, SCHEMA_VERSION,
};
use serde_json::Value;

// ---------------------------------------------------------------------------
// The taxonomy itself
// ---------------------------------------------------------------------------

/// The table, written out. #9 pins these numbers and the strings beside them:
/// a caller branches on them, so changing one is a breaking change to this
/// tool's interface and has to be a deliberate edit here rather than a
/// consequence of reordering an enum.
const TAXONOMY: &[(ErrorKind, &str, u8)] = &[
    (ErrorKind::InvalidInput, "invalid_input", 2),
    (ErrorKind::BrowserLaunchFailed, "browser_launch_failed", 10),
    (
        ErrorKind::ChromeDownloadFailed,
        "chrome_download_failed",
        11,
    ),
    (ErrorKind::NavigationTimeout, "navigation_timeout", 20),
    (ErrorKind::NavigationFailed, "navigation_failed", 21),
    (ErrorKind::CloudflareBlocked, "cloudflare_blocked", 22),
    (ErrorKind::ProductNotFound, "product_not_found", 23),
    (
        ErrorKind::EmptyPageOrCatalogEnd,
        "empty_page_or_catalog_end",
        24,
    ),
    (ErrorKind::NetworkError, "network_error", 30),
    (ErrorKind::IoError, "io_error", 31),
    (ErrorKind::CacheError, "cache_error", 32),
    (ErrorKind::JsonError, "json_error", 40),
    (ErrorKind::ParseFailed, "parse_failed", 41),
    (ErrorKind::Internal, "internal_error", 70),
];

#[test]
fn the_exit_code_table_is_what_the_readme_documents() {
    assert_eq!(
        TAXONOMY.len(),
        ErrorKind::ALL.len(),
        "a variant was added to ErrorKind without a row in the documented table"
    );

    for (kind, error_type, exit_code) in TAXONOMY {
        assert_eq!(kind.error_type(), *error_type);
        assert_eq!(kind.exit_code(), *exit_code);
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
            IherbError::Navigation("Failed to navigate to https://x: timeout".into()),
            ErrorKind::NavigationTimeout,
        ),
        (
            IherbError::Navigation("Failed to get page content: closed".into()),
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
        // No code of its own: `--currency` named something this storefront does
        // not price in, and the only thing that changes the answer is a
        // different flag.
        (
            IherbError::CurrencyMismatch {
                expected: "CHF".into(),
                actual: "USD".into(),
                what: "product 12949".into(),
            },
            ErrorKind::InvalidInput,
        ),
        (IherbError::Cache("nope".into()), ErrorKind::CacheError),
        (
            IherbError::Io(std::io::Error::other("nope")),
            ErrorKind::IoError,
        ),
        (
            IherbError::Json(serde_json::from_str::<Value>("{").unwrap_err()),
            ErrorKind::JsonError,
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
#[test]
fn a_cached_document_dates_the_page_earlier_than_the_run() {
    let config = config("no", Some("NOK"));
    let emitted = at(1_756_000_000);
    let fetched = at(1_756_000_000 - 3 * 24 * 60 * 60);

    let meta = Meta::new(
        &config,
        Some(Provenance {
            fetched_at: fetched,
            from_cache: true,
        }),
        emitted,
    );

    assert_eq!(meta.from_cache, Some(true));
    assert_eq!(meta.fetched_at.as_deref(), Some("2025-08-21T01:46:40Z"));
    assert_eq!(meta.emitted_at, "2025-08-24T01:46:40Z");
    assert!(meta.fetched_at.as_deref().unwrap() < meta.emitted_at.as_str());
}

#[test]
fn a_fresh_document_dates_the_page_and_the_run_alike() {
    let config = config("no", Some("NOK"));
    let now = at(1_756_000_000);

    let meta = Meta::new(
        &config,
        Some(Provenance {
            fetched_at: now,
            from_cache: false,
        }),
        now,
    );

    assert_eq!(meta.from_cache, Some(false));
    assert_eq!(meta.fetched_at.as_deref(), Some(meta.emitted_at.as_str()));
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
    assert_eq!(meta.storefront.as_deref(), Some("https://no.iherb.com"));
}

/// A command line clap refused has no effective storefront, and inventing one
/// would be a claim about a run that never started.
#[test]
fn a_run_that_never_configured_claims_no_storefront() {
    let meta = Meta::unconfigured(None, at(1_756_000_000));
    assert_eq!(meta.country, None);
    assert_eq!(meta.currency, None);
    assert_eq!(meta.storefront, None);
    assert_eq!(meta.tool_version, env!("CARGO_PKG_VERSION"));
}

/// The storefront in `meta` is the one the run resolved to, and this is the
/// case that has to be right for a non-US corpus: a US-shaped assumption would
/// hide exactly here.
#[test]
fn the_envelope_names_a_non_usd_storefront() {
    let config = config("no", Some("NOK"));
    let meta = Meta::new(&config, None, at(1_756_000_000));

    assert_eq!(meta.country.as_deref(), Some("no"));
    assert_eq!(meta.currency.as_deref(), Some("NOK"));
    assert_eq!(meta.storefront.as_deref(), Some("https://no.iherb.com"));
}

/// `--currency` is a question the run asked the storefront, not an answer about
/// a price. A run that asked nothing says `null` here — and the currency a
/// price is actually in lives on the record, with its provenance (#5).
#[test]
fn asking_for_no_currency_claims_no_currency() {
    let config = config("no", None);
    let meta = Meta::new(&config, None, at(1_756_000_000));

    assert_eq!(meta.currency, None);
    assert_eq!(meta.storefront.as_deref(), Some("https://no.iherb.com"));
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
        assert_eq!(document["meta"]["storefront"], "https://no.iherb.com");
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

/// `meta.country` is the value the run *resolved* to, not the flag it was
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
    assert_eq!(meta["country"], "no");
    assert_eq!(meta["currency"], "NOK");
    assert_eq!(meta["storefront"], "https://no.iherb.com");
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
