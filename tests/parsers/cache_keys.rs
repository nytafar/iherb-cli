//! Cache identity: which fetches share an entry on disk.
//!
//! `CacheKey::file_name` is pinned directly, and the two collisions that #1 and
//! #6 are about are demonstrated through the layer that actually carries the
//! request context — `ProductTarget` and `SearchTarget`, built from two
//! `AppConfig`s that differ — and then through a real `Cache` round-trip in a
//! temp directory. `CacheKey` itself cannot express a country or a limit, so
//! asserting on it alone could only ever compare a value to itself.

use std::path::PathBuf;

use iherb_cli::cache::{Cache, CacheKey};
use iherb_cli::cli::SortOrder;
use iherb_cli::config::AppConfig;
use iherb_cli::fetch::FetchTarget;
use iherb_cli::model::{ProductDetail, ProductSummary, SearchResult};
use iherb_cli::targets::{ProductTarget, SearchTarget};

use crate::fixture::TempDir;

fn product(id: &str) -> String {
    CacheKey::Product {
        product_id: id.to_string(),
    }
    .file_name()
}

fn search(query: &str, sort: SortOrder, category: Option<&str>) -> String {
    CacheKey::Search {
        query: query.to_string(),
        sort,
        category: category.map(str::to_string),
    }
    .file_name()
}

/// A config that differs from its sibling only in storefront.
fn config(country: &str, currency: &str, cache_dir: PathBuf) -> AppConfig {
    AppConfig {
        country: country.to_string(),
        currency: currency.to_string(),
        no_cache: false,
        delay_ms: 0,
        debug: false,
        browser_path: None,
        cache_dir,
        data_dir: PathBuf::from("/nonexistent"),
    }
}

// ---------------------------------------------------------------------------
// The derivation itself
// ---------------------------------------------------------------------------

#[test]
fn product_entries_are_named_after_the_product_id() {
    assert_eq!(product("61864"), "product_61864.json");
    assert_ne!(product("61864"), product("61865"));
}

#[test]
fn search_entries_are_a_hash_of_query_sort_and_category() {
    let base = search("magnesium", SortOrder::Relevance, None);
    assert!(base.starts_with("search_"));
    assert!(base.ends_with(".json"));
    // 16 hex characters between the prefix and the extension.
    assert_eq!(base.len(), "search_".len() + 16 + ".json".len());

    assert_eq!(base, search("magnesium", SortOrder::Relevance, None));
    assert_ne!(
        base,
        search("magnesium citrate", SortOrder::Relevance, None)
    );
    assert_ne!(base, search("magnesium", SortOrder::Rating, None));
    assert_ne!(
        base,
        search("magnesium", SortOrder::Relevance, Some("107703"))
    );
    assert_ne!(
        search("magnesium", SortOrder::Relevance, Some("107703")),
        search("magnesium", SortOrder::Relevance, Some("101022"))
    );
}

// ---------------------------------------------------------------------------
// #1 — the country collision
// ---------------------------------------------------------------------------

/// CHARACTERIZATION, NOT DESIRED: pins #1. Two configs that differ only in
/// `--country` produce targets that fetch *different URLs* and land on the
/// *same cache file*.
///
/// #1 flips this: the last assertion becomes `assert_ne!`, because the key
/// gains a country (and a `v2_` prefix so poisoned entries are abandoned
/// rather than reused). Do not change the file naming to satisfy this test;
/// fix the test when #1 lands.
#[test]
fn two_storefronts_share_one_product_cache_file() {
    let dir = TempDir::new("country-key");
    let us = config("us", "USD", dir.path());
    let ch = config("ch", "CHF", dir.path());

    let from_us = ProductTarget::new(&us, "61864").unwrap();
    let from_ch = ProductTarget::new(&ch, "61864").unwrap();

    // The request really does differ...
    assert_eq!(from_us.url(1), "https://www.iherb.com/pr/item/61864");
    assert_eq!(from_ch.url(1), "https://ch.iherb.com/pr/item/61864");
    assert_ne!(from_us.url(1), from_ch.url(1));

    // ...and the cache cannot tell the two apart.
    assert_eq!(
        from_us.cache_key().file_name(),
        from_ch.cache_key().file_name()
    );
}

/// CHARACTERIZATION, NOT DESIRED: the same #1, shown as the failure a user
/// actually hits — the Swiss storefront being handed the US price, with no
/// error and a plausible `Data from` timestamp.
///
/// #1 flips this: the Swiss read becomes a miss, so `get` returns `None` and
/// the `expect` below fails.
#[test]
fn a_swiss_fetch_is_served_the_us_price() {
    let dir = TempDir::new("country-roundtrip");
    let us = config("us", "USD", dir.path());
    let ch = config("ch", "CHF", dir.path());
    let cache = Cache::new(dir.path(), false);

    let us_data = ProductDetail {
        name: "California Gold Nutrition, Gold C®, 1,000 mg, 60 Veggie Capsules".to_string(),
        brand: "California Gold Nutrition".to_string(),
        price: 9.60,
        original_price: None,
        currency: "USD".to_string(),
        rating: Some(4.8),
        review_count: Some(381_864),
        product_url: "https://www.iherb.com/pr/item/61864".to_string(),
        product_id: "61864".to_string(),
        in_stock: Some(true),
        description: None,
        product_code: None,
        upc: None,
        ingredients: None,
        supplement_facts: None,
        suggested_use: None,
        warnings: None,
        shipping_weight: None,
        category_breadcrumb: None,
        review_distribution: None,
        // Hand-built, not extracted: `Strategy::Unrecorded` with no sources.
        extraction: Default::default(),
    };

    let us_key = ProductTarget::new(&us, "61864").unwrap().cache_key();
    cache.set(&us_key, &us_data).expect("write the US entry");
    assert_eq!(dir.file_count(), 1);

    let ch_key = ProductTarget::new(&ch, "61864").unwrap().cache_key();
    let served = cache
        .get::<ProductDetail>(&ch_key)
        .expect("the Swiss read hits the US entry")
        .data;

    // A CHF request comes back with a USD price labelled USD, from a URL on
    // the US storefront. Nothing in the value says it is the wrong storefront.
    assert_eq!(served.currency, "USD");
    assert_eq!(served.price, 9.60);
    assert_eq!(served.product_url, "https://www.iherb.com/pr/item/61864");
    assert_eq!(dir.file_count(), 1, "one storefront, one file, both reads");
}

/// CHARACTERIZATION, NOT DESIRED: the search half of #1 — same query, two
/// storefronts, one entry. Flipped by the same fix as the two above.
#[test]
fn two_storefronts_share_one_search_cache_file() {
    let dir = TempDir::new("country-search-key");
    let us = config("us", "USD", dir.path());
    let de = config("de", "EUR", dir.path());

    let from_us = SearchTarget::new(&us, "magnesium", 20, SortOrder::Rating, None).unwrap();
    let from_de = SearchTarget::new(&de, "magnesium", 20, SortOrder::Rating, None).unwrap();

    assert_ne!(from_us.url(1), from_de.url(1));
    assert_eq!(
        from_us.cache_key().file_name(),
        from_de.cache_key().file_name()
    );
}

// ---------------------------------------------------------------------------
// #6 — the limit collision
// ---------------------------------------------------------------------------

/// CHARACTERIZATION, NOT DESIRED: pins the storage half of #6. Two searches
/// that differ only in `--limit` plan a different number of page fetches and
/// still land on the same entry.
///
/// #6 flips this **if** it takes the key-based resolution ("treat it as a
/// miss and refetch"): the two names diverge and this becomes `assert_ne!`.
/// If instead it records the fetched page count inside the entry and merges on
/// read, the key stays shared on purpose and this test is replaced by the
/// round-trip below — which fails under either resolution.
#[test]
fn two_limits_share_one_search_cache_file() {
    let dir = TempDir::new("limit-key");
    let cfg = config("us", "USD", dir.path());

    let ten = SearchTarget::new(&cfg, "magnesium", 10, SortOrder::Relevance, None).unwrap();
    let two_hundred =
        SearchTarget::new(&cfg, "magnesium", 200, SortOrder::Relevance, None).unwrap();

    // A different amount of work is planned...
    assert_eq!(ten.page_count(), 1);
    assert_eq!(two_hundred.page_count(), 5);

    // ...for the same entry.
    assert_eq!(
        ten.cache_key().file_name(),
        two_hundred.cache_key().file_name()
    );
}

/// CHARACTERIZATION, NOT DESIRED: #6 as the user hits it. A `--limit 10` run
/// caches the 48 products one page yielded; the later `--limit 200` run reads
/// that entry and is handed 48, with nothing in the value recording that only
/// one page was ever fetched.
///
/// #6 flips this under either resolution: a key that includes the limit turns
/// the read into a miss, and an entry that records its page count adds a field
/// to what is stored, which breaks the literal below. Fix the test when #6
/// lands; do not widen the cache to satisfy it.
#[test]
fn a_widened_search_is_served_the_narrow_runs_results() {
    let dir = TempDir::new("limit-roundtrip");
    let cfg = config("us", "USD", dir.path());
    let cache = Cache::new(dir.path(), false);

    // What one result page yields, which is what a `--limit 10` run fetches.
    let one_page = SearchResult {
        query: "magnesium".to_string(),
        total_results: Some(12_008),
        products: (0..48)
            .map(|i| ProductSummary {
                name: format!("Magnesium {}", i),
                brand: "Acme".to_string(),
                price: 1.0,
                original_price: None,
                currency: "USD".to_string(),
                rating: None,
                review_count: None,
                product_url: format!("https://www.iherb.com/pr/p/{}", i),
                product_id: i.to_string(),
                in_stock: true,
            })
            .collect(),
    };

    let narrow = SearchTarget::new(&cfg, "magnesium", 10, SortOrder::Relevance, None).unwrap();
    cache
        .set(&narrow.cache_key(), &one_page)
        .expect("write the narrow entry");

    let wide = SearchTarget::new(&cfg, "magnesium", 200, SortOrder::Relevance, None).unwrap();
    let served = cache
        .get::<SearchResult>(&wide.cache_key())
        .expect("the widened read hits the narrow entry")
        .data;

    assert_eq!(wide.limit(), 200);
    assert_eq!(served.products.len(), 48, "asked for 200, cache holds 48");
    // ...and the header will still say "of 12,008", so the shortfall is
    // invisible to the caller.
    assert_eq!(served.total_results, Some(12_008));
    assert_eq!(dir.file_count(), 1);
}
