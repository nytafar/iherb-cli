//! Cache identity: which fetches share an entry on disk.
//!
//! `CacheKey::file_name` is pinned directly, and the two collisions that #1 and
//! #6 are about are demonstrated through the layer that actually carries the
//! request context — `ProductTarget` and `SearchTarget`, built from two
//! `AppConfig`s that differ — and then through a real `Cache` round-trip in a
//! temp directory. `CacheKey` could not express a country or a limit, so
//! asserting on it alone could only ever compare a value to itself; #1 gave it
//! the country, and the round-trips below are what proves the country reaches
//! it from a config rather than only from a hand-built key.

use std::path::PathBuf;

use iherb_cli::cache::{Cache, CacheKey};
use iherb_cli::cli::SortOrder;
use iherb_cli::config::AppConfig;
use iherb_cli::fetch::FetchTarget;
use iherb_cli::model::{ProductDetail, ProductSummary, SearchResult};
use iherb_cli::targets::{ProductTarget, SearchTarget};

use crate::fixture::TempDir;

fn product(country: &str, id: &str) -> String {
    CacheKey::Product {
        country: country.to_string(),
        product_id: id.to_string(),
    }
    .file_name()
}

fn search(country: &str, query: &str, sort: SortOrder, category: Option<&str>) -> String {
    CacheKey::Search {
        country: country.to_string(),
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
fn product_entries_are_named_after_the_storefront_and_the_product_id() {
    assert_eq!(product("us", "61864"), "v2_product_us_61864.json");
    assert_ne!(product("us", "61864"), product("us", "61865"));
    assert_ne!(product("us", "61864"), product("ch", "61864"));
}

/// The `v2_` generation is not decoration: it is what stops a `v1` entry,
/// written before the key knew about storefronts, from being read now. Nothing
/// deletes those files; they are simply never named again.
#[test]
fn v1_entries_can_never_be_named_by_the_current_key() {
    let dir = TempDir::new("v1-orphaned");
    let us = config("us", "USD", dir.path());
    let cache = Cache::new(dir.path(), false);

    // A poisoned entry exactly as v1 wrote it, with a mtime of now so the TTL
    // cannot be what saves us.
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(dir.path().join("product_61864.json"), r#""stale""#).unwrap();

    let key = ProductTarget::new(&us, "61864").unwrap().cache_key();
    assert_ne!(key.file_name(), "product_61864.json");
    assert!(cache.get::<String>(&key).is_none(), "v1 entry was read");
}

#[test]
fn search_entries_are_a_hash_of_country_query_sort_and_category() {
    let base = search("us", "magnesium", SortOrder::Relevance, None);
    assert!(base.starts_with("v2_search_"));
    assert!(base.ends_with(".json"));
    // 16 hex characters between the prefix and the extension.
    assert_eq!(base.len(), "v2_search_".len() + 16 + ".json".len());

    assert_eq!(base, search("us", "magnesium", SortOrder::Relevance, None));
    assert_ne!(base, search("ch", "magnesium", SortOrder::Relevance, None));
    assert_ne!(
        base,
        search("us", "magnesium citrate", SortOrder::Relevance, None)
    );
    assert_ne!(base, search("us", "magnesium", SortOrder::Rating, None));
    assert_ne!(
        base,
        search("us", "magnesium", SortOrder::Relevance, Some("107703"))
    );
    assert_ne!(
        search("us", "magnesium", SortOrder::Relevance, Some("107703")),
        search("us", "magnesium", SortOrder::Relevance, Some("101022"))
    );

    // Every field is NUL-delimited, so no two distinct requests can hash alike
    // by running together at a field boundary...
    assert_ne!(
        search("us", "magnesium", SortOrder::Relevance, None),
        search("u", "smagnesium", SortOrder::Relevance, None)
    );

    // ...and the optional field is tagged, so an empty category is not the
    // same request as no category. Same failure class as #1, one storefront
    // smaller.
    assert_ne!(
        search("us", "magnesium", SortOrder::Relevance, None),
        search("us", "magnesium", SortOrder::Relevance, Some(""))
    );
}

// ---------------------------------------------------------------------------
// #1 — the country collision
// ---------------------------------------------------------------------------

/// FLIPPED BY #1. This asserted the collision: two configs differing only in
/// `--country` produced targets that fetched *different URLs* and landed on
/// the *same cache file*. The key now carries the country, so different
/// requests get different entries.
#[test]
fn two_storefronts_get_their_own_product_cache_file() {
    let dir = TempDir::new("country-key");
    let us = config("us", "USD", dir.path());
    let ch = config("ch", "CHF", dir.path());

    let from_us = ProductTarget::new(&us, "61864").unwrap();
    let from_ch = ProductTarget::new(&ch, "61864").unwrap();

    // The request really does differ...
    assert_eq!(from_us.url(1), "https://www.iherb.com/pr/item/61864");
    assert_eq!(from_ch.url(1), "https://ch.iherb.com/pr/item/61864");
    assert_ne!(from_us.url(1), from_ch.url(1));

    // ...and so does the entry it is filed under.
    assert_ne!(
        from_us.cache_key().file_name(),
        from_ch.cache_key().file_name()
    );
    assert_eq!(from_us.cache_key().file_name(), "v2_product_us_61864.json");
    assert_eq!(from_ch.cache_key().file_name(), "v2_product_ch_61864.json");
}

/// FLIPPED BY #1, and the one that matters: the failure a user actually hit
/// was the Swiss storefront being handed the US price, with no error and a
/// plausible `Data from` timestamp, for the 30 days of the TTL. The Swiss read
/// is now a miss, so the caller fetches Switzerland instead of being told a
/// USD price is a Swiss one.
#[test]
fn a_swiss_fetch_is_not_served_the_us_price() {
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
    assert!(
        cache.get::<ProductDetail>(&ch_key).is_none(),
        "the Swiss read was served the US entry"
    );

    // The US entry is untouched and still readable by the request that wrote
    // it: this is a miss for Switzerland, not a cache that stopped working.
    let still_there = cache
        .get::<ProductDetail>(&us_key)
        .expect("the US read still hits its own entry")
        .data;
    assert_eq!(still_there.currency, "USD");
    assert_eq!(still_there.price, 9.60);

    // Once Switzerland is written it is a second file, not an overwrite.
    cache.set(&ch_key, &us_data).expect("write the Swiss entry");
    assert_eq!(dir.file_count(), 2, "two storefronts, two files");
}

/// FLIPPED BY #1: the search half — same query, two storefronts, which used to
/// be one entry and is now two.
#[test]
fn two_storefronts_get_their_own_search_cache_file() {
    let dir = TempDir::new("country-search-key");
    let us = config("us", "USD", dir.path());
    let de = config("de", "EUR", dir.path());

    let from_us = SearchTarget::new(&us, "magnesium", 20, SortOrder::Rating, None).unwrap();
    let from_de = SearchTarget::new(&de, "magnesium", 20, SortOrder::Rating, None).unwrap();

    assert_ne!(from_us.url(1), from_de.url(1));
    assert_ne!(
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
