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
use iherb_cli::model::{ProductDetail, ProductSummary, SearchFetch, SearchResult};
use iherb_cli::targets::{ProductTarget, SearchTarget};

use crate::fixture::TempDir;

fn product(country: &str, id: &str) -> String {
    product_in(country, None, id)
}

/// A product entry's name, for a run that asked for a particular currency (#5).
fn product_in(country: &str, currency: Option<&str>, id: &str) -> String {
    CacheKey::Product {
        country: country.to_string(),
        currency: currency.map(str::to_string),
        product_id: id.to_string(),
    }
    .file_name()
}

fn search(country: &str, query: &str, sort: SortOrder, category: Option<&str>) -> String {
    search_in(country, None, query, sort, category)
}

fn search_in(
    country: &str,
    currency: Option<&str>,
    query: &str,
    sort: SortOrder,
    category: Option<&str>,
) -> String {
    CacheKey::Search {
        country: country.to_string(),
        currency: currency.map(str::to_string),
        query: query.to_string(),
        sort,
        category: category.map(str::to_string),
    }
    .file_name()
}

/// A config that differs from its sibling only in storefront, or only in the
/// currency `--currency` requires.
fn config(country: &str, currency: &str, cache_dir: PathBuf) -> AppConfig {
    AppConfig {
        country: country.to_string(),
        currency: Some(currency.to_string()),
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
    assert_eq!(product("us", "61864"), "v4_product_us_any_61864.json");
    assert_ne!(product("us", "61864"), product("us", "61865"));
    assert_ne!(product("us", "61864"), product("ch", "61864"));
}

/// The generation prefix is not decoration: it is what stops an entry written
/// under an older set of rules from being read now. Nothing deletes those
/// files; they are simply never named again.
#[test]
fn entries_from_older_generations_can_never_be_named_by_the_current_key() {
    let dir = TempDir::new("older-generations-orphaned");
    let us = config("us", "USD", dir.path());
    let cache = Cache::new(dir.path(), false);

    // Poisoned entries exactly as v1 and v2 wrote them, with a mtime of now so
    // the TTL cannot be what saves us.
    std::fs::create_dir_all(dir.path()).unwrap();
    for stale in [
        "product_61864.json",
        "v2_product_us_61864.json",
        "v3_product_us_61864.json",
    ] {
        std::fs::write(dir.path().join(stale), r#""stale""#).unwrap();
    }

    let key = ProductTarget::new(&us, "61864").unwrap().cache_key();
    assert_eq!(key.file_name(), "v4_product_us_USD_61864.json");
    assert!(
        cache.get::<String>(&key).is_none(),
        "a stale entry was read"
    );
}

/// FLIPPED BY #5's second half: two currencies are two entries, because they
/// are two documents.
///
/// This test asserted the opposite one commit ago, and the reasoning was sound
/// for the code that existed then. `--currency` was an assertion about the
/// storefront: it could reject an answer but could not change which document
/// was fetched, so keying on it would have filed one fetch under two names.
///
/// The cookie changed the fact the reasoning rested on. `--currency` now sets
/// iHerb's own storefront preference before the request, so the same product id
/// really does come back as a different document — NOK 880.63, €76.57 and
/// $64.56 for product 12949. Two documents under one name is #1's bug, one
/// dimension over, and the `v4_` generation is what abandons the `v3_` entries
/// that were written under the old, currency-blind name.
#[test]
fn two_currencies_get_their_own_cache_file() {
    let dir = TempDir::new("currency-key");
    let usd = config("us", "USD", dir.path());
    let chf = config("us", "CHF", dir.path());

    let from_usd = ProductTarget::new(&usd, "61864").unwrap();
    let from_chf = ProductTarget::new(&chf, "61864").unwrap();

    // The URL is the same — the currency is carried on a cookie, not in the
    // path — which is exactly why the key has to say it. Nothing about the
    // request that reaches the network is visible in `url()`.
    assert_eq!(from_usd.url(1), from_chf.url(1));
    assert_ne!(
        from_usd.cache_key().file_name(),
        from_chf.cache_key().file_name()
    );
    assert_eq!(
        from_usd.cache_key().file_name(),
        "v4_product_us_USD_61864.json"
    );
    assert_eq!(
        from_chf.cache_key().file_name(),
        "v4_product_us_CHF_61864.json"
    );

    // And asking for nothing is its own request, distinct from asking for the
    // currency the storefront happens to default to: one sets the preference
    // cookies and one does not.
    assert_eq!(product("us", "61864"), "v4_product_us_any_61864.json");
    assert_ne!(product("us", "61864"), from_usd.cache_key().file_name());

    // `any` is a sentinel, not a currency, and it cannot be spoofed: every
    // currency that reaches the key has been upper-cased.
    assert_ne!(
        product_in("us", Some("ANY"), "61864"),
        product("us", "61864")
    );
}

/// The search half of the same thing.
#[test]
fn two_currencies_get_their_own_search_cache_file() {
    let base = search("us", "magnesium", SortOrder::Relevance, None);
    let in_usd = search_in("us", Some("USD"), "magnesium", SortOrder::Relevance, None);
    let in_nok = search_in("us", Some("NOK"), "magnesium", SortOrder::Relevance, None);

    assert_ne!(base, in_usd, "asking for nothing is not asking for USD");
    assert_ne!(in_usd, in_nok);

    // NUL-delimited like every other field, so no two distinct requests hash
    // alike by running together at the boundary.
    assert_ne!(
        search_in("us", Some("USD"), "magnesium", SortOrder::Relevance, None),
        search_in("u", Some("SUSD"), "magnesium", SortOrder::Relevance, None)
    );
}

#[test]
fn search_entries_are_a_hash_of_country_query_sort_and_category() {
    let base = search("us", "magnesium", SortOrder::Relevance, None);
    assert!(base.starts_with("v4_search_"));
    assert!(base.ends_with(".json"));
    // 16 hex characters between the prefix and the extension.
    assert_eq!(base.len(), "v4_search_".len() + 16 + ".json".len());

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
    assert_eq!(
        from_us.cache_key().file_name(),
        "v4_product_us_USD_61864.json"
    );
    assert_eq!(
        from_ch.cache_key().file_name(),
        "v4_product_ch_CHF_61864.json"
    );
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
        currency: Some("USD".to_string()),
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
    assert_eq!(still_there.currency.as_deref(), Some("USD"));
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

/// Two searches that differ only in `--limit` share one entry, and after #6
/// they still do — deliberately, not by oversight.
///
/// #6 offered two resolutions. Under the key-based one this would have become
/// `assert_ne!`. It took the other: the entry records what its walk did, and
/// the search path decides whether what is stored answers the request. Keeping
/// the key shared is the point of that resolution — one entry holds everything
/// either run fetched, so a narrow run can be served out of a wide run's work
/// instead of fetching the same pages again under a second name.
///
/// The behaviour that used to be missing is not here. It cannot be: this file
/// tests the cache layer, and the cache layer is still dumb — it hands back the
/// entry it was asked for, which is what `a_widened_search_still_finds_the_narrow_entry`
/// below shows. What changed lives in the search path, in
/// `search::a_cached_search_short_of_the_limit_is_refetched`.
#[test]
fn two_limits_share_one_search_cache_file() {
    let dir = TempDir::new("limit-key");
    let cfg = config("us", "USD", dir.path());

    let ten = SearchTarget::new(&cfg, "magnesium", 10, SortOrder::Relevance, None).unwrap();
    let two_hundred =
        SearchTarget::new(&cfg, "magnesium", 200, SortOrder::Relevance, None).unwrap();

    // A different amount of work is budgeted...
    assert_eq!(ten.page_count(), 2);
    assert_eq!(two_hundred.page_count(), 6);

    // ...for the same entry.
    assert_eq!(
        ten.cache_key().file_name(),
        two_hundred.cache_key().file_name()
    );
}

/// The cache layer after #6: it still hands back the entry it was asked for,
/// short or not, because deciding whether an entry answers a request is not its
/// job. What the entry now carries is a record of the walk that wrote it —
/// `pages_fetched` and `exhausted` — which is what lets the layer above tell a
/// record that is short because iHerb has no more from one that is short
/// because the run that wrote it did not ask for more.
///
/// The behavioural half of #6 is `search::a_cached_search_short_of_the_limit_is_refetched`.
/// It has to live there: this file cannot express it, since under the
/// resolution #6 took the cache read is *supposed* to succeed here.
#[test]
fn a_widened_search_still_finds_the_narrow_entry() {
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
                price: Some(1.0),
                original_price: None,
                currency: Some("USD".to_string()),
                rating: None,
                review_count: None,
                product_url: format!("https://www.iherb.com/pr/p/{}", i),
                product_id: i.to_string(),
                in_stock: Some(true),
                extraction: Default::default(),
            })
            .collect(),
        // What a `--limit 10` run leaves behind: one page walked, and iHerb
        // nowhere near out of results.
        fetch: SearchFetch {
            pages_fetched: Some(1),
            exhausted: Some(false),
        },
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
    assert_eq!(served.products.len(), 48, "asked for 200, entry holds 48");
    assert_eq!(served.total_results, Some(12_008));
    assert_eq!(dir.file_count(), 1);

    // The entry says how it came to hold only 48, which is what the search path
    // reads. Before #6 there was nothing here to read, so a short entry and a
    // complete one were the same value.
    assert_eq!(served.fetch.pages_fetched, Some(1));
    assert_eq!(served.fetch.exhausted, Some(false));
}
