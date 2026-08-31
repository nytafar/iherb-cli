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
//! Adding a page: drop `<slug>.html.gz` in `tests/fixtures/` and add one line to
//! [`GZIPPED`] plus one `pub const` below.

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
}

/// California Gold Nutrition Two a Day. JSON-LD prices arrive as a
/// `priceSpecification` array with a strikethrough entry.
pub const TWO_A_DAY: Fixture = Fixture {
    slug: "product-104996-cgn-two-a-day",
    product_id: "104996",
};

/// California Gold Nutrition B Complex. JSON-LD carries a flat top-level price.
pub const B_COMPLEX: Fixture = Fixture {
    slug: "product-108255-cgn-b-complex",
    product_id: "108255",
};

/// OLLY Goodbye Stress gummies — the awkward one. Out of stock, no review
/// distribution widget, and no `.prodOverviewIngred`.
pub const OLLY_GUMMIES: Fixture = Fixture {
    slug: "product-119174-olly-gummies",
    product_id: "119174",
};

/// Nordic Naturals Ultimate Omega softgels.
pub const ULTIMATE_OMEGA: Fixture = Fixture {
    slug: "product-12949-nordic-ultimate-omega",
    product_id: "12949",
};

/// California Gold Nutrition Gold C powder.
pub const GOLD_C_POWDER: Fixture = Fixture {
    slug: "product-59561-cgn-gold-c-powder",
    product_id: "59561",
};

/// `/search?kw=vitamin+c`, 48 cards, "1 - 48 of 11,952 results".
pub const SEARCH_VITAMIN_C: Fixture = Fixture {
    slug: "search-vitamin-c",
    product_id: "",
};

/// `/c/supplements`, a category listing. Not parsed by anything yet; kept for
/// the catalog command in #21.
pub const CATEGORY_SUPPLEMENTS: Fixture = Fixture {
    slug: "category-supplements",
    product_id: "",
};

/// Every page the suite can load. Used by tests that sweep all of them.
pub const ALL: &[Fixture] = &[
    TWO_A_DAY,
    B_COMPLEX,
    OLLY_GUMMIES,
    ULTIMATE_OMEGA,
    GOLD_C_POWDER,
    SEARCH_VITAMIN_C,
    CATEGORY_SUPPLEMENTS,
];

/// The five product detail pages.
pub const PRODUCTS: &[Fixture] = &[
    TWO_A_DAY,
    B_COMPLEX,
    OLLY_GUMMIES,
    ULTIMATE_OMEGA,
    GOLD_C_POWDER,
];

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
pub const BASE_URL: &str = "https://www.iherb.com";

/// Load a JSON side-fixture from `tests/fixtures/<name>.json`.
///
/// Two parsers are fed JSON that never comes from the page HTML:
/// `parse_from_js_globals` reads globals the browser evaluates, and
/// `parse_from_next_data` reads a `__NEXT_DATA__` block. See the files
/// themselves for where each one's contents came from.
pub fn json(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/{}.json",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {}", path, e));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {}", path, e))
}

/// Every gzipped page, as `(slug, bytes)`. `include_bytes!` keeps the suite
/// independent of the working directory.
const GZIPPED: &[(&str, &[u8])] = &[
    (
        "product-104996-cgn-two-a-day",
        include_bytes!("../fixtures/product-104996-cgn-two-a-day.html.gz"),
    ),
    (
        "product-108255-cgn-b-complex",
        include_bytes!("../fixtures/product-108255-cgn-b-complex.html.gz"),
    ),
    (
        "product-119174-olly-gummies",
        include_bytes!("../fixtures/product-119174-olly-gummies.html.gz"),
    ),
    (
        "product-12949-nordic-ultimate-omega",
        include_bytes!("../fixtures/product-12949-nordic-ultimate-omega.html.gz"),
    ),
    (
        "product-59561-cgn-gold-c-powder",
        include_bytes!("../fixtures/product-59561-cgn-gold-c-powder.html.gz"),
    ),
    (
        "search-vitamin-c",
        include_bytes!("../fixtures/search-vitamin-c.html.gz"),
    ),
    (
        "category-supplements",
        include_bytes!("../fixtures/category-supplements.html.gz"),
    ),
];

fn inflated() -> &'static HashMap<&'static str, String> {
    static CACHE: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        GZIPPED
            .iter()
            .map(|&(slug, gz)| {
                let mut out = String::new();
                flate2::read::GzDecoder::new(gz)
                    .read_to_string(&mut out)
                    .unwrap_or_else(|e| panic!("{}.html.gz is not valid gzip: {}", slug, e));
                (slug, out)
            })
            .collect()
    })
}

/// Compare a rendered string against `tests/fixtures/golden/<name>.md`.
///
/// Run with `UPDATE_GOLDEN=1` to rewrite the file instead of asserting, then
/// read the diff before committing it.
pub fn assert_golden(name: &str, actual: &str) {
    let path = format!(
        "{}/tests/fixtures/golden/{}.md",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
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
