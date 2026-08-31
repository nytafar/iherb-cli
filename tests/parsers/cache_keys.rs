//! `CacheKey::file_name` — the derivation that decides which fetches share a
//! cache entry.
//!
//! The file name is load-bearing on disk: changing it orphans every entry users
//! already have, so it is pinned here rather than left to be rediscovered.

use iherb_cli::cache::CacheKey;
use iherb_cli::cli::SortOrder;

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

/// CHARACTERIZATION, NOT DESIRED: pins the #1 bug. `CacheKey` has no country
/// field, so `--country us` and `--country ch` land on the same file and the
/// second storefront is served the first one's prices for the 30-day TTL.
///
/// #1 flips this: the key gains a country (and a `v2_` prefix so poisoned
/// entries are abandoned rather than reused). Do not rename these files to
/// satisfy this test; fix the test when #1 lands.
#[test]
fn cache_keys_are_blind_to_country_and_currency() {
    // There is no country to vary — the type cannot express one. The name a US
    // fetch writes is the name a Swiss fetch reads.
    assert_eq!(product("61864"), "product_61864.json");
    assert_eq!(
        search("magnesium", SortOrder::Relevance, None),
        search("magnesium", SortOrder::Relevance, None)
    );
}

/// CHARACTERIZATION, NOT DESIRED: pins the storage half of #6. The key ignores
/// `--limit`, so `--limit 10` and `--limit 200` share one entry, and the wider
/// call is served whatever the narrower one happened to fetch.
///
/// #6 flips this: either the key records how much was fetched, or the read
/// path refetches when the cached entry is short. Fix the test when #6 lands.
#[test]
fn cache_keys_are_blind_to_limit() {
    // As with country, `limit` is not part of `CacheKey` at all: `SearchTarget`
    // holds it, and it never reaches the file name.
    let ten = search("magnesium", SortOrder::Relevance, None);
    let two_hundred = search("magnesium", SortOrder::Relevance, None);
    assert_eq!(ten, two_hundred);
}
