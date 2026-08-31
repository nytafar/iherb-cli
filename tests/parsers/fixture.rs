//! The fixture loader.
//!
//! Seven real iHerb pages live gzipped under `tests/fixtures/`. This module
//! inflates them once per test binary and hands each parser the shape it wants,
//! because the parsers do not agree on one:
//!
//! | want | call |
//! |---|---|
//! | raw HTML (`parse_from_html`, `parse_search_from_html`) | [`Fixture::html`] |
//! | a parsed document (`extract_spec`, `parse_supplement_facts_html`, …) | [`Fixture::doc`] |
//! | the JSON-LD blob (`parse_from_json_ld`) | [`Fixture::json_ld`] |
//! | a JSON side-fixture (`parse_from_next_data`, `parse_from_js_globals`) | [`json`] |
//!
//! Adding a page is **one line**: drop `<slug>.html.gz` in `tests/fixtures/`
//! and add a row to the [`registry!`] block below. The named constant, the
//! `all()` and `products()` sweeps and the gzip bytes are all derived from that
//! row — there is no second list to keep in step.

use std::collections::HashMap;
use std::io::Read;
use std::sync::OnceLock;

use scraper::Html;
use serde_json::Value;

/// A captured page. Cheap to copy; the bytes are shared and inflated once.
#[derive(Clone, Copy, Debug)]
pub struct Fixture {
    /// File stem under `tests/fixtures/`, without `.html.gz`.
    slug: &'static str,
    /// The iHerb product id the page was captured for, or `""` for pages that
    /// are not a single product.
    product_id: &'static str,
    /// The page as committed. `include_bytes!` keeps the suite independent of
    /// the working directory.
    gz: &'static [u8],
}

/// Declares each page once: a named constant, its slug, and its product id.
/// Everything else about a fixture is derived from the row.
macro_rules! registry {
    ($( $(#[$meta:meta])* $name:ident = $slug:literal, $product_id:literal; )*) => {
        $(
            $(#[$meta])*
            pub const $name: Fixture = Fixture {
                slug: $slug,
                product_id: $product_id,
                gz: include_bytes!(concat!("../fixtures/", $slug, ".html.gz")),
            };
        )*

        /// Every registered page, in declaration order.
        const REGISTRY: &[Fixture] = &[$($name),*];
    };
}

registry! {
    /// California Gold Nutrition Two a Day. JSON-LD prices arrive as a
    /// `priceSpecification` array with a strikethrough entry, and this is the
    /// only capture with a populated review histogram.
    TWO_A_DAY = "product-104996-cgn-two-a-day", "104996";

    /// California Gold Nutrition B Complex. JSON-LD carries a flat top-level
    /// price; its review-histogram element is an empty shell.
    B_COMPLEX = "product-108255-cgn-b-complex", "108255";

    /// OLLY Goodbye Stress gummies — the awkward one. Out of stock, no review
    /// histogram element at all, and no `.prodOverviewIngred`.
    OLLY_GUMMIES = "product-119174-olly-gummies", "119174";

    /// Nordic Naturals Ultimate Omega softgels. The page the JS-globals side
    /// fixture was transcribed from.
    ULTIMATE_OMEGA = "product-12949-nordic-ultimate-omega", "12949";

    /// California Gold Nutrition Gold C powder. A one-nutrient supplement
    /// table; its review-histogram element is an empty shell.
    GOLD_C_POWDER = "product-59561-cgn-gold-c-powder", "59561";

    /// `/search?kw=vitamin+c`, 48 cards, "1 - 48 of 11,952 results", and the
    /// sort dropdown and category facets that #3 and #4 are about.
    SEARCH_VITAMIN_C = "search-vitamin-c", "";

    /// `/c/supplements`, a category listing. Not parsed by anything yet; kept
    /// for the catalog command in #21.
    CATEGORY_SUPPLEMENTS = "category-supplements", "";
}

/// Every page the suite can load, for tests that sweep all of them.
pub fn all() -> impl Iterator<Item = Fixture> {
    REGISTRY.iter().copied()
}

/// The product detail pages — every registered page that names a product.
pub fn products() -> impl Iterator<Item = Fixture> {
    REGISTRY
        .iter()
        .copied()
        .filter(|f| !f.product_id.is_empty())
}

impl Fixture {
    pub fn slug(self) -> &'static str {
        self.slug
    }

    /// The product id to hand parsers that take one.
    pub fn product_id(self) -> &'static str {
        assert!(
            !self.product_id.is_empty(),
            "{} is not a product page",
            self.slug
        );
        self.product_id
    }

    /// The page as captured. Inflated once per test binary.
    pub fn html(self) -> &'static str {
        inflated()
            .get(self.slug)
            .unwrap_or_else(|| panic!("no fixture registered for {}", self.slug))
            .as_str()
    }

    /// The page parsed. Parsing is ~100 ms on these pages, so a test that needs
    /// the document more than once should bind it, not call this twice.
    pub fn doc(self) -> Html {
        Html::parse_document(self.html())
    }

    /// The `application/ld+json` `Product` blob, via the same extractor
    /// production uses.
    pub fn json_ld(self) -> Value {
        iherb_cli::scraper::extract::extract_json_ld(self.html())
            .unwrap_or_else(|| panic!("{} has no JSON-LD Product block", self.slug))
    }
}

/// A document with nothing in it, for asserting the empty-input path.
pub fn empty_doc() -> Html {
    Html::parse_document("<html><body></body></html>")
}

/// The base URL every parser test passes, so expected URLs read the same way.
/// It matches the storefront the fixtures were captured from: `countryCode`
/// is `US` on all seven.
pub const BASE_URL: &str = "https://www.iherb.com";

/// Load a JSON side-fixture from `tests/fixtures/<name>.json`.
///
/// Two parsers are fed JSON that never comes from the page HTML:
/// `parse_from_js_globals` reads globals the browser evaluates, and
/// `parse_from_next_data` reads a `__NEXT_DATA__` block no captured page has.
/// See the files themselves for where each one's contents came from.
pub fn json(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/{}.json",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {}", path, e));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {}", path, e))
}

fn inflated() -> &'static HashMap<&'static str, String> {
    static CACHE: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        REGISTRY
            .iter()
            .map(|f| {
                let mut out = String::new();
                flate2::read::GzDecoder::new(f.gz)
                    .read_to_string(&mut out)
                    .unwrap_or_else(|e| panic!("{}.html.gz is not valid gzip: {}", f.slug, e));
                (f.slug, out)
            })
            .collect()
    })
}

/// Compare a rendered string against `tests/fixtures/golden/<name>.md`.
///
/// Run with `UPDATE_GOLDEN=1` to rewrite the file instead of asserting, then
/// read the diff before committing it. Any other value — `0` included — still
/// asserts, so a stale `UPDATE_GOLDEN=0` in a shell profile cannot silently
/// turn the golden tests into a rubber stamp.
pub fn assert_golden(name: &str, actual: &str) {
    let path = format!(
        "{}/tests/fixtures/golden/{}.md",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
        std::fs::create_dir_all(format!(
            "{}/tests/fixtures/golden",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("create golden dir");
        std::fs::write(&path, actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {}. Re-run with UPDATE_GOLDEN=1 to create it.", path, e));
    assert_eq!(
        expected, actual,
        "{} drifted. Re-run with UPDATE_GOLDEN=1 and review the diff.",
        name
    );
}

/// A scratch directory that removes itself, for the cache tests. Not a general
/// tempdir: it panics rather than reporting, which is what a test wants.
pub struct TempDir(std::path::PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "iherb-cli-test-{}-{}-{}",
            label,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    pub fn path(&self) -> std::path::PathBuf {
        self.0.clone()
    }

    /// How many files the directory holds. One cache file where two were
    /// expected is the whole point of the #1 tests.
    pub fn file_count(&self) -> usize {
        std::fs::read_dir(&self.0)
            .expect("read temp dir")
            .filter_map(Result::ok)
            .count()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
