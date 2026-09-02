//! The fixture loader.
//!
//! Twenty real iHerb pages live gzipped under `tests/fixtures/`. This module
//! inflates them once per test binary and hands each parser the shape it wants,
//! because the parsers do not agree on one:
//!
//! | want | call |
//! |---|---|
//! | raw HTML (`parse_from_html`, `parse_search_from_html`) | [`Fixture::html`] |
//! | a parsed document (`extract_spec`, `parse_supplement_facts_html`, …) | [`Fixture::doc`] |
//! | the JSON-LD blob (`parse_from_json_ld`) | [`Fixture::json_ld`] |
//! | a JSON side-fixture (`parse_from_js_globals`) | [`json`] |
//!
//! Adding a page is **one line**: drop `<slug>.html.gz` in `tests/fixtures/`
//! and add a row to the [`registry!`] block below. The named constant, the
//! `all()` and `products()` sweeps and the gzip bytes are all derived from that
//! row — there is no second list to keep in step.
//!
//! A row also names the **storefront** the page came from. It used to be safe
//! to assume one: every capture was `www.iherb.com` in USD, so the sweeps wrote
//! `"USD"` and `BASE_URL` as literals. #5 added a Norwegian capture, and a
//! literal in a sweep is a claim about every page rather than about the one it
//! is looking at. [`Fixture::currency`] and [`Fixture::base_url`] are what the
//! sweeps ask instead.
//!
//! The registry is in two halves, and `tests/fixtures/README.md` is where the
//! difference is argued. The first eight rows are the **legacy regression
//! corpus**: a US snapshot from an upstream fork, kept because tests are pinned
//! to it. The rest are the **current corpus**, captured by this repository from
//! the Norwegian storefront for the products this tool is actually used for,
//! and they are what new parser work should be designed against.

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
    /// The ISO code the page's own storefront prices in — what
    /// `detect_currency_from_html` must return for it, and what every parser
    /// sweep compares against instead of a hardcoded `"USD"`.
    currency: &'static str,
    /// The storefront the page was served from, to pass parsers that build
    /// absolute URLs. Not always `https://www.iherb.com` since #5.
    base_url: &'static str,
    /// The page as committed. `include_bytes!` keeps the suite independent of
    /// the working directory.
    gz: &'static [u8],
}

/// Declares each page once: a named constant, its slug, and its product id.
/// Everything else about a fixture is derived from the row.
macro_rules! registry {
    ($( $(#[$meta:meta])* $name:ident = $slug:literal, $product_id:literal, $currency:literal, $base_url:expr; )*) => {
        $(
            $(#[$meta])*
            pub const $name: Fixture = Fixture {
                slug: $slug,
                product_id: $product_id,
                currency: $currency,
                base_url: $base_url,
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
    TWO_A_DAY = "product-104996-cgn-two-a-day", "104996", "USD", US_STOREFRONT;

    /// California Gold Nutrition B Complex. JSON-LD carries a flat top-level
    /// price; its review-histogram element is an empty shell.
    B_COMPLEX = "product-108255-cgn-b-complex", "108255", "USD", US_STOREFRONT;

    /// OLLY Goodbye Stress gummies — the awkward one. Out of stock, no review
    /// histogram element at all, and no `.prodOverviewIngred`.
    OLLY_GUMMIES = "product-119174-olly-gummies", "119174", "USD", US_STOREFRONT;

    /// Nordic Naturals Ultimate Omega softgels. The page the JS-globals side
    /// fixture was transcribed from.
    ULTIMATE_OMEGA = "product-12949-nordic-ultimate-omega", "12949", "USD", US_STOREFRONT;

    /// The same product as [`ULTIMATE_OMEGA`], from the Norwegian storefront in
    /// NOK (#5).
    ///
    /// The suite's only non-USD page, and the only one this repository captured
    /// itself. It is here because every sweep that compared a currency to the
    /// literal `"USD"` was really asserting that iHerb has one storefront: they
    /// passed for seven pages that happened to agree, and could not have failed.
    /// Being the *same product* as the USD capture is the point — the price
    /// differs because the storefront differs, not because the product does.
    ULTIMATE_OMEGA_NOK = "product-12949-nordic-ultimate-omega-nok", "12949", "NOK", NO_STOREFRONT;

    /// California Gold Nutrition Gold C powder. A one-nutrient supplement
    /// table; its review-histogram element is an empty shell.
    GOLD_C_POWDER = "product-59561-cgn-gold-c-powder", "59561", "USD", US_STOREFRONT;

    /// `/search?kw=vitamin+c`, 48 cards, "1 - 48 of 11,952 results", and the
    /// sort dropdown and category facets that #3 and #4 are about.
    SEARCH_VITAMIN_C = "search-vitamin-c", "", "USD", US_STOREFRONT;

    /// `/c/supplements`, a category listing. Not parsed by anything yet; kept
    /// for the catalog command in #21.
    CATEGORY_SUPPLEMENTS = "category-supplements", "", "USD", US_STOREFRONT;

    /// `/search?kw=vitamin+d3&sr=4` on the Norwegian storefront — price
    /// ascending, which is the ordering that puts iHerb's unpriced listings
    /// first.
    ///
    /// Captured for #56 and #57. Nine of its cards are discontinued and carry
    /// `data-ga-discount-price="0"` beside `data-ga-is-discontinued="True"`,
    /// and the first three of them are the whole grid's opening rows. It is the
    /// only page in the corpus with an out-of-stock *card* — [`OLLY_GUMMIES`]
    /// is an out-of-stock product page, which is a different extractor — so it
    /// is what pins both the zero-as-absent rule and the search view's
    /// availability line against something iHerb actually served.
    SEARCH_VITAMIN_D3_PRICE_ASC = "search-vitamin-d3-price-asc-nok", "", "NOK", NO_STOREFRONT;

    // -----------------------------------------------------------------------
    // The current corpus (#8): the Norwegian storefront, and the products this
    // tool is actually used for. Twelve pages captured 2026-09-01 at
    // siteVersion 1.0.22698 — roughly 2,600 builds newer than the seven above,
    // which are a US snapshot from another fork.
    //
    // Chosen to span *forms*, because that is what #15's structured-quantity
    // and container model has to be designed against, and the eight pages above
    // offer veggie caps, gummies, softgels and one 250 g powder — no tablet, no
    // micro tablet, no delayed-release cap, no liquid, and nothing dosed in
    // anything but milligrams.
    // -----------------------------------------------------------------------

    /// Swanson FiberAid larch arabinogalactan, 250 g. A **powder sold by the
    /// gram**, whose derived unit is kr/g rather than kr/capsule.
    FIBERAID_POWDER = "product-118148-swanson-fiberaid-arabinogalactan-nok", "118148", "NOK", NO_STOREFRONT;

    /// Biocidin Dentalcidin toothpaste, 90 ml tube. The corpus's first
    /// **liquid measured in millilitres**, and the first page for a product
    /// that is not swallowed at all: no serving size, no capsule count, and a
    /// supplement-facts panel that a toothpaste has no reason to carry.
    DENTALCIDIN_TUBE = "product-143499-biocidin-dentalcidin-toothpaste-nok", "143499", "NOK", NO_STOREFRONT;

    /// Allergy Research Group ButyrEn, 100 **delayed-release** vegetarian
    /// capsules. The release mechanism is part of the form, and the form string
    /// is where it is stated.
    BUTYREN_DELAYED = "product-35060-arg-butyren-tributyrin-nok", "35060", "NOK", NO_STOREFRONT;

    /// KAL Lithium Orotate, 90 **micro tablets**. Neither "tablet" nor
    /// "capsule": a third unit noun, on a page that also carries a flavour
    /// ("Lemon Lime") in the product title.
    LITHIUM_MICRO_TABLETS = "product-78419-kal-lithium-orotate-nok", "78419", "NOK", NO_STOREFRONT;

    /// Swanson Supreme C-Complex, 250 **tablets**. A six-ingredient blend on
    /// the corpus's first tablet page.
    SUPREME_C_TABLETS = "product-117699-swanson-supreme-c-complex-nok", "117699", "NOK", NO_STOREFRONT;

    /// Nutricost Vitamin K2 MK-7, 240 softgels. A **single-nutrient** page for
    /// contrast with the blends, and one dosed in micrograms.
    K2_SOFTGELS = "product-124094-nutricost-k2-mk7-nok", "124094", "NOK", NO_STOREFRONT;

    /// Enzymedica Digest Gold, 240 capsules. Eleven enzymes dosed in **activity
    /// units** — DU, HUT, AGU, CU, FIP, ALU, GalU, SU, BGU, XU, HCU — and not
    /// one of them a mass. Anything that assumes a supplement-facts amount
    /// parses as `<number> <mass unit>` meets its counterexample here.
    DIGEST_GOLD_UNITS = "product-16790-enzymedica-digest-gold-nok", "16790", "NOK", NO_STOREFRONT;

    /// BodyBio Calcium/Magnesium Butyrate, 250 capsules — **125 servings**.
    /// The serving is two capsules, so the container count and the serving
    /// count are different numbers on the same page. #15 has to model both.
    BUTYRATE_TWO_CAP_SERVING = "product-105890-bodybio-calcium-magnesium-butyrate-nok", "105890", "NOK", NO_STOREFRONT;

    /// Country Life Coenzyme B-Complex, 240 vegan capsules. A twelve-nutrient
    /// blend mixing mg, mcg and mcg DFE in one facts panel.
    COENZYME_B_COMPLEX = "product-12081-country-life-coenzyme-b-complex-nok", "12081", "NOK", NO_STOREFRONT;

    /// Dynamic Health Tart Cherry Concentrate, 946 ml. The second liquid, and
    /// the **ingestible** one: a volume dosed by the tablespoon, with the size
    /// stated in both fl oz and ml.
    TART_CHERRY_LIQUID = "product-75722-dynamic-health-tart-cherry-nok", "75722", "NOK", NO_STOREFRONT;

    /// Doctor's Best Stabilized R-Lipoic Acid, 60 veggie caps. Product id
    /// **`4`** — one digit. Every other id in this suite is five or six, and an
    /// id-shaped assumption that has only ever seen those is unfalsifiable
    /// without this page. It also carries 8,555 reviews, the largest count here.
    R_LIPOIC_TINY_ID = "product-4-doctors-best-r-lipoic-acid-nok", "4", "NOK", NO_STOREFRONT;

    /// Humanx Lactobacillus Gasseri & Reuteri+, 30 veggie capsules. Dosed in
    /// **CFU**, which is a count of organisms and not a quantity of anything
    /// weighable.
    GASSERI_REUTERI_CFU = "product-132364-humanx-gasseri-reuteri-nok", "132364", "NOK", NO_STOREFRONT;

    // -----------------------------------------------------------------------
    // Two hand-authored Supplement Facts panels, captured 2026-09-02 for #65.
    //
    // Every panel above is generated markup, and all twenty of them write their
    // column header the same way. These two do not, which is how a header row
    // reached `nutrients` on a real run: they were both in the 113-product
    // boswellia comparison that found it.
    // -----------------------------------------------------------------------

    /// Vitacost Root2 Boswellia Serrata, 60 capsules. Its facts panel labels
    /// the first column **`Nutrient`** — a header cell with a word in it, where
    /// every other capture leaves that cell blank (#65).
    ROOT2_NAMED_HEADER = "product-159500-vitacost-root2-boswellia-nok", "159500", "NOK", NO_STOREFRONT;

    /// Vitacost Synergy 5-Loxin Boswellia Extract, 120 capsules. The same
    /// header row with a **zero-width space** (`U+200B`) where the word would
    /// be: invisible, not whitespace by Unicode's reckoning, and therefore not
    /// removed by `trim()` (#65).
    ///
    /// Its one nutrient name is also split by a `<br>`, so it is what pins the
    /// cell text being joined with a space rather than with nothing.
    LOXIN_ZERO_WIDTH_HEADER = "product-159125-vitacost-5-loxin-boswellia-nok", "159125", "NOK", NO_STOREFRONT;
}

/// iHerb's own not-found page, US storefront, captured 2026-09-02 (#59).
///
/// `https://www.iherb.com/pr/item/99999999` — an id the catalogue has never
/// had. Twelve kilobytes: a header, a search box, an error panel and a link
/// home. It is here because `is_not_found_page` checked three copy strings the
/// site does not serve, and nothing noticed for as long as no capture of this
/// page existed to notice with.
///
/// It is also the corpus's only page carrying Cloudflare's `challenge-platform`
/// bootstrap *and* nothing else — see [`NOT_FOUND_NO`] for the same page
/// without it.
pub const NOT_FOUND_US: Fixture = Fixture {
    slug: "notfound-product-99999999",
    product_id: "99999999",
    currency: "",
    base_url: US_STOREFRONT,
    gz: include_bytes!("../fixtures/notfound-product-99999999.html.gz"),
};

/// The same page from the Norwegian storefront, captured in the same minute.
///
/// Byte-identical to [`NOT_FOUND_US`] apart from the hostname in three links —
/// including the title, which is English on both. That is the finding, not a
/// redundancy: a marker list checked against one storefront would have been a
/// guess about the other, and this is what makes the pair of them a measurement.
pub const NOT_FOUND_NO: Fixture = Fixture {
    slug: "notfound-product-99999999-nok",
    product_id: "99999999",
    currency: "",
    base_url: NO_STOREFRONT,
    gz: include_bytes!("../fixtures/notfound-product-99999999-nok.html.gz"),
};

/// A Cloudflare managed challenge — **synthesized, not captured** (#23).
///
/// This programme has never received a live challenge: 28 searches and 12
/// captures, not one interstitial, so clearance is *unmeasured* rather than
/// confirmed and there is no real page to commit here. The alternative to a
/// synthetic fixture was no positive test at all, and the alternative to saying
/// so is a suite that looks like it has seen something it has not.
///
/// Reconstructed from Cloudflare's published managed-challenge page: the
/// `#challenge-running` heading, the `#challenge-stage` wrapper, the
/// `.cf-turnstile` widget and its `challenges.cloudflare.com` iframe, the
/// `#challenge-form` POST, the `window._cf_chl_opt` block, and the footer's
/// Ray ID and "Performance & security by Cloudflare" line. Every value that
/// would be a real token reads `SYNTHETIC`.
///
/// **What it proves:** that a page shaped like Cloudflare's own is classified
/// `cloudflare_blocked` rather than falling through to extraction and being
/// reported as a missing product. **What it does not prove:** that iHerb's
/// challenge, when one finally arrives, is shaped like this one. Only a capture
/// can prove that, and when one exists it belongs here beside this file rather
/// than replacing it.
pub const CHALLENGE_SYNTHETIC: Fixture = Fixture {
    slug: "cloudflare-managed-challenge-synthetic",
    product_id: "",
    currency: "",
    base_url: US_STOREFRONT,
    gz: include_bytes!("../fixtures/cloudflare-managed-challenge-synthetic.html.gz"),
};

/// Pages that load like any other fixture but must never enter a sweep.
///
/// [`all`] means "every page a parser is expected to read", and every sweep
/// written against it asserts something no error page can satisfy: that it
/// declares a currency, that it is not mistaken for a 404. None of these is a
/// product page or a listing, so each is addressed by name —
/// [`NOT_FOUND_US`], [`NOT_FOUND_NO`], [`CHALLENGE_SYNTHETIC`] — and inflated
/// alongside the rest.
///
/// [`CHALLENGE_SYNTHETIC`] carries a second reason: it is the one file here
/// this repository did not receive from a server, and letting it into a sweep
/// would put a page nobody has seen served on the same footing as
/// twenty-three that were.
const OFF_REGISTRY: &[Fixture] = &[NOT_FOUND_US, NOT_FOUND_NO, CHALLENGE_SYNTHETIC];

/// Both not-found captures, for the tests that must hold on either storefront.
pub fn not_found_pages() -> impl Iterator<Item = Fixture> {
    [NOT_FOUND_US, NOT_FOUND_NO].into_iter()
}

/// Every page iHerb actually served to this repository.
///
/// [`all`] plus the two error captures, and pointedly **not**
/// [`CHALLENGE_SYNTHETIC`], which nobody received. A sweep asserting "this is
/// what the real site looks like" has to be able to say which files that
/// covers.
pub fn pages_iherb_served() -> impl Iterator<Item = Fixture> {
    all().chain(not_found_pages())
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

    /// The currency this page's storefront prices in.
    pub fn currency(self) -> &'static str {
        self.currency
    }

    /// The storefront this page was served from.
    pub fn base_url(self) -> &'static str {
        self.base_url
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

/// The US storefront, which seven of the eight captures came from.
pub const US_STOREFRONT: &str = "https://www.iherb.com";

/// The Norwegian storefront, which the NOK capture came from (#5).
pub const NO_STOREFRONT: &str = "https://no.iherb.com";

/// The base URL a test passes when the storefront is not what it is testing, so
/// expected URLs read the same way across the file.
///
/// Prefer [`Fixture::base_url`] wherever a test hands a *fixture* to a parser:
/// this constant is right for the seven US pages and wrong for the Norwegian
/// one, and a test that passes it anyway is asserting against a URL the page
/// never had.
pub const BASE_URL: &str = US_STOREFRONT;

/// Load a JSON side-fixture from `tests/fixtures/<name>.json`.
///
/// One parser is fed JSON that never comes from the page HTML:
/// `parse_from_js_globals` reads globals the browser evaluates rather than
/// anything the HTML carries. See `tests/fixtures/README.md` for where the
/// side-fixture's contents came from.
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
            .chain(OFF_REGISTRY.iter())
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
