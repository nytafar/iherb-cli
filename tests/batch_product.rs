//! Batch `product` fetches (#10): many ids, one browser session, NDJSON
//! streaming under `--json`, per-item error isolation.
//!
//! Process-level, like the equivalent half of `json_output.rs` — only a
//! separate process can answer "is stdout really NDJSON" the way it answers
//! "is stdout really one document": the shape of what actually reached the
//! pipe, not a reading of the code that was supposed to produce it. Every run
//! here is answered entirely from a seeded cache or from argument validation,
//! so none of it needs a real browser or the network.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use iherb_cli::cache::CacheKey;
use iherb_cli::model::{Extraction, ProductDetail, Source, Strategy};

/// A throwaway `$HOME`, mirroring `json_output.rs`'s `Home` — kept separate
/// rather than shared, because this file's runs need stdin plumbing and
/// environment overrides that file has no use for.
struct Home(PathBuf);

impl Home {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "iherb-cli-batch-{}-{}-{}",
            label,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create temp home");
        Self(path)
    }

    #[cfg(target_os = "macos")]
    fn cache_dir(&self) -> PathBuf {
        self.0.join("Library/Caches/iherb-cli")
    }

    #[cfg(not(target_os = "macos"))]
    fn cache_dir(&self) -> PathBuf {
        self.0.join(".cache/iherb-cli")
    }

    /// Seed a cache entry for `product_id` on the Norwegian storefront in
    /// NOK, so a batch that names it answers from disk and never starts
    /// Chrome.
    fn seed_product(&self, product_id: &str) {
        let dir = self.cache_dir();
        std::fs::create_dir_all(&dir).expect("create cache dir");
        let key = CacheKey::Product {
            country: "no".to_string(),
            currency: Some("NOK".to_string()),
            product_id: product_id.to_string(),
        };
        std::fs::write(
            dir.join(key.file_name()),
            serde_json::to_string_pretty(&a_product(product_id)).expect("serialize the seed"),
        )
        .expect("write cache entry");
    }

    /// Run the binary with the given ids on stdin and no browser path that
    /// could ever resolve — see [`Self::run`] for why.
    fn run_stdin(&self, args: &[&str], stdin: &str) -> Ran {
        self.spawn(args, Some(stdin), NO_BROWSER)
    }

    /// Run the binary against `IHERB_BROWSER_PATH` set to a file that does
    /// not exist.
    ///
    /// A path named there binds (#55): if the run ever tries to resolve
    /// Chrome, it fails loudly with `invalid_input` rather than silently
    /// falling through to a real, working browser. Every test in this file
    /// runs this way, so a batch that unexpectedly launches Chrome — the one
    /// thing the acceptance criteria say a fully-cached batch must never do —
    /// fails immediately instead of passing by accident because a real
    /// browser happened to be on the machine running the test.
    fn run(&self, args: &[&str]) -> Ran {
        self.spawn(args, None, NO_BROWSER)
    }

    fn spawn(&self, args: &[&str], stdin: Option<&str>, browser_path: &str) -> Ran {
        let mut command = Command::new(env!("CARGO_BIN_EXE_iherb-cli"));
        command
            .args(args)
            .env("HOME", &self.0)
            .env("IHERB_BROWSER_PATH", browser_path)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_CACHE_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("IHERB_COUNTRY")
            .env_remove("IHERB_CURRENCY")
            .env_remove("RUST_LOG")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = if let Some(stdin) = stdin {
            command.stdin(Stdio::piped());
            let mut child = command.spawn().expect("spawn iherb-cli");
            child
                .stdin
                .take()
                .expect("child stdin was piped")
                .write_all(stdin.as_bytes())
                .expect("write to child stdin");
            child.wait_with_output().expect("run iherb-cli")
        } else {
            command.stdin(Stdio::null());
            command.output().expect("run iherb-cli")
        };

        Ran {
            code: output.status.code().expect("the process was not signalled"),
            stdout: String::from_utf8(output.stdout).expect("stdout is UTF-8"),
            stderr: String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        }
    }
}

/// A path nothing will ever resolve to a real Chrome. See [`Home::run`].
const NO_BROWSER: &str = "/nonexistent/not-a-real-browser";

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
    /// Every stdout line, parsed as its own JSON value — the NDJSON claim
    /// itself, not an assumption behind the tests that read further into it.
    /// Only valid for a genuine batch run: a single-document run is pretty-
    /// printed and spans several lines, so use [`Ran::document`] for that.
    fn lines(&self) -> Vec<serde_json::Value> {
        self.stdout
            .lines()
            .map(|line| {
                serde_json::from_str(line).unwrap_or_else(|e| {
                    panic!(
                        "line is not one JSON value ({e}): {line:?}\nfull stdout:\n{}",
                        self.stdout
                    )
                })
            })
            .collect()
    }

    /// The one JSON document on stdout, for a run that never entered the
    /// batch pipeline: a whole-batch failure, or a single id.
    fn document(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "stdout is not exactly one JSON document ({e}); it was:\n{}",
                self.stdout
            )
        })
    }
}

fn a_product(product_id: &str) -> ProductDetail {
    let mut product = ProductDetail {
        name: format!("Test Product {}", product_id),
        brand: "Test Brand".to_string(),
        price: 123.0,
        original_price: None,
        currency: Some("NOK".to_string()),
        rating: Some(4.5),
        review_count: Some(10),
        product_url: format!("https://no.iherb.com/pr/{}", product_id),
        product_id: product_id.to_string(),
        in_stock: None,
        description: Some("A product.".to_string()),
        product_code: None,
        upc: None,
        ingredients: None,
        supplement_facts: None,
        suggested_use: None,
        warnings: None,
        shipping_weight: None,
        category_breadcrumb: None,
        review_distribution: None,
        extraction: Extraction::new(Strategy::JsonLd),
    };
    product.claim_unattributed(Source::JsonLd);
    product
}

fn storefront_args<'a>() -> &'a [&'a str] {
    &["--country", "no", "--currency", "NOK"]
}

// ---------------------------------------------------------------------------
// Argument validation: whole-batch failures never stream, never touch a page
// ---------------------------------------------------------------------------

/// `--stdin` together with ids on the command line is two lists to reconcile,
/// and this tool refuses rather than guessing which one wins. This is a
/// *whole-batch* failure: nothing was ever attempted, so it answers with the
/// ordinary single-document envelope, not NDJSON.
#[test]
fn stdin_and_positional_ids_together_is_invalid_input() {
    let home = Home::new("stdin-and-ids");
    let ran = home.run(&["product", "143499", "--stdin", "--json"]);

    assert_eq!(ran.code, 2, "stderr was:\n{}", ran.stderr);
    let document = ran.document();
    assert_eq!(document["ok"], false);
    assert_eq!(document["error_type"], "invalid_input");
}

/// No ids at all — neither on the command line nor on stdin — is the same
/// kind of failure, for the same reason.
#[test]
fn no_ids_at_all_is_invalid_input() {
    let home = Home::new("no-ids");
    let ran = home.run(&["product", "--json"]);

    assert_eq!(ran.code, 2, "stderr was:\n{}", ran.stderr);
    let document = ran.document();
    assert_eq!(document["ok"], false);
    assert_eq!(document["error_type"], "invalid_input");
}

/// Blank lines on stdin do not count as ids; an input of only blank lines is
/// the same as no ids.
#[test]
fn blank_stdin_lines_are_not_ids() {
    let home = Home::new("blank-stdin");
    let mut args = vec!["product", "--stdin", "--json"];
    args.extend_from_slice(storefront_args());
    let ran = home.run_stdin(&args, "\n\n   \n");

    assert_eq!(ran.code, 2, "stderr was:\n{}", ran.stderr);
    assert_eq!(ran.document()["error_type"], "invalid_input");
}

/// `--concurrency 0` cannot fetch anything; refused before any target is
/// touched.
#[test]
fn zero_concurrency_is_invalid_input() {
    let home = Home::new("zero-concurrency");
    let mut args = vec!["product", "143499", "479", "--concurrency", "0", "--json"];
    args.extend_from_slice(storefront_args());
    let ran = home.run(&args);

    assert_eq!(ran.code, 2, "stderr was:\n{}", ran.stderr);
    assert_eq!(ran.document()["error_type"], "invalid_input");
}

// ---------------------------------------------------------------------------
// The single-id path is unchanged: still one pretty-printed document
// ---------------------------------------------------------------------------

/// One id, no `--stdin`, is not "a batch of one" — it is the original
/// single-document contract, verbatim. Pretty-printed (multi-line), not the
/// compact NDJSON shape a real batch would use for the same id.
#[test]
fn a_single_id_is_still_one_pretty_printed_document() {
    let home = Home::new("single-id");
    home.seed_product("143499");
    let mut args = vec!["product", "143499", "--json"];
    args.extend_from_slice(storefront_args());
    let ran = home.run(&args);

    assert_eq!(ran.code, 0, "stderr was:\n{}", ran.stderr);
    assert!(
        ran.stdout.starts_with("{\n"),
        "a single id must render the original pretty-printed envelope, not a \
         batch line; stdout was:\n{}",
        ran.stdout
    );
    let document = ran.document();
    assert_eq!(document["data"]["product_id"], "143499");
}

// ---------------------------------------------------------------------------
// The batch pipeline: NDJSON, cache short-circuit, per-item isolation
// ---------------------------------------------------------------------------

/// Two ids, both cache hits: two NDJSON lines, each the same envelope shape a
/// single document carries — `ok`, `schema_version`, `meta`, `data` — plus a
/// `product_id` naming which line is which. Compact, one JSON value per line,
/// which is what makes it NDJSON rather than two pretty documents concatenated.
///
/// Run with `IHERB_BROWSER_PATH` bound to a path that cannot resolve (see
/// [`Home::run`]): if this ever tried to launch Chrome for either id, it
/// would fail loudly rather than quietly using a real browser this
/// environment happens to have. It exits 0, which is the proof a browser was
/// never touched — the acceptance criterion the issue states directly.
#[test]
fn a_fully_cached_batch_streams_ndjson_and_never_starts_chrome() {
    let home = Home::new("cached-batch");
    home.seed_product("143499");
    home.seed_product("479");
    let mut args = vec!["product", "143499", "479", "--json"];
    args.extend_from_slice(storefront_args());
    let ran = home.run(&args);

    assert_eq!(ran.code, 0, "stderr was:\n{}", ran.stderr);

    let lines = ran.lines();
    assert_eq!(lines.len(), 2, "stdout was:\n{}", ran.stdout);
    for line in &lines {
        assert_eq!(line["ok"], true);
        assert_eq!(line["schema_version"], 1);
        assert!(line["meta"]["requested_storefront"].is_string());
        assert!(line["data"].is_object());
    }
    let ids: Vec<&str> = lines
        .iter()
        .map(|l| l["product_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"143499"));
    assert!(ids.contains(&"479"));

    // NDJSON, not pretty JSON: each raw line is compact, and the whole of
    // stdout is not itself parseable as a single document.
    let raw_lines: Vec<&str> = ran.stdout.lines().collect();
    assert_eq!(raw_lines.len(), 2);
    for raw in &raw_lines {
        assert!(
            !raw.contains('\n'),
            "an NDJSON line must not itself span lines"
        );
    }
    assert!(
        serde_json::from_str::<serde_json::Value>(&ran.stdout).is_err(),
        "two NDJSON lines must not also parse as one JSON document"
    );
}

/// A batch delivered through `--stdin`, exactly like the pipeline the issue
/// names: `search --json | jq -r '...product_id' | product --stdin --json`.
#[test]
fn stdin_ids_drive_the_same_batch_pipeline() {
    let home = Home::new("stdin-batch");
    home.seed_product("143499");
    home.seed_product("479");
    let mut args = vec!["product", "--stdin", "--json"];
    args.extend_from_slice(storefront_args());
    let ran = home.run_stdin(&args, "143499\n479\n");

    assert_eq!(ran.code, 0, "stderr was:\n{}", ran.stderr);
    let lines = ran.lines();
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|l| l["ok"] == true));
}

/// `--stdin` with exactly one id still takes the batch pipeline — asking for
/// it is asking for streaming output, whatever the count turns out to be.
#[test]
fn stdin_with_one_id_is_still_the_batch_pipeline() {
    let home = Home::new("stdin-one");
    home.seed_product("143499");
    let mut args = vec!["product", "--stdin", "--json"];
    args.extend_from_slice(storefront_args());
    let ran = home.run_stdin(&args, "143499\n");

    assert_eq!(ran.code, 0, "stderr was:\n{}", ran.stderr);
    assert!(
        !ran.stdout.starts_with("{\n"),
        "--stdin must take the batch (compact NDJSON) shape even for one id; \
         stdout was:\n{}",
        ran.stdout
    );
    let lines = ran.lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["product_id"], "143499");
}

/// **A bad id does not take the batch with it.** One cache hit and one id
/// that is neither numeric nor a URL: the batch reports both, the good line
/// unaffected by the bad one, and the run still exits `0` because something
/// came back.
#[test]
fn a_bad_id_fails_its_own_line_and_the_batch_keeps_going() {
    let home = Home::new("mixed-batch");
    home.seed_product("143499");
    let mut args = vec!["product", "143499", "not-a-valid-id", "--json"];
    args.extend_from_slice(storefront_args());
    let ran = home.run(&args);

    assert_eq!(
        ran.code, 0,
        "at least one id succeeded, so the batch as a whole is not a failure; stderr was:\n{}",
        ran.stderr
    );

    let lines = ran.lines();
    assert_eq!(lines.len(), 2, "stdout was:\n{}", ran.stdout);

    let good = lines
        .iter()
        .find(|l| l["product_id"] == "143499")
        .expect("the cached id's line");
    assert_eq!(good["ok"], true);

    let bad = lines
        .iter()
        .find(|l| l["product_id"] == "not-a-valid-id")
        .expect("the bad id's own line");
    assert_eq!(bad["ok"], false);
    assert_eq!(bad["error_type"], "invalid_input");
    assert!(bad["data"].is_null());
}

/// **Every id failing is the one case the batch itself reports as a
/// failure.** Two bad ids, both `invalid_input` (exit 2): the batch exits 2
/// rather than 0, so a caller who never inspects individual lines still
/// learns that nothing came back.
#[test]
fn a_batch_that_fails_everywhere_exits_on_the_worst_code() {
    let home = Home::new("all-fail-batch");
    let mut args = vec!["product", "not-an-id-1", "not-an-id-2", "--json"];
    args.extend_from_slice(storefront_args());
    let ran = home.run(&args);

    assert_eq!(ran.code, 2, "stderr was:\n{}", ran.stderr);
    let lines = ran.lines();
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|l| l["ok"] == false));
    assert!(lines.iter().all(|l| l["error_type"] == "invalid_input"));
}

/// A batch still honours `--section`, exactly as the single-id path does: the
/// projection is decided once and both pipelines read the same answer.
#[test]
fn a_batch_honours_section() {
    let home = Home::new("batch-section");
    home.seed_product("143499");
    home.seed_product("479");
    let mut args = vec![
        "product",
        "143499",
        "479",
        "--section",
        "description",
        "--json",
    ];
    args.extend_from_slice(storefront_args());
    let ran = home.run(&args);

    assert_eq!(ran.code, 0, "stderr was:\n{}", ran.stderr);
    for line in ran.lines() {
        assert!(line["data"]["description"].is_string());
        assert!(
            line["data"].get("price").is_none(),
            "a projected batch line must narrow its keys exactly as the \
             single-id path does"
        );
    }
}
