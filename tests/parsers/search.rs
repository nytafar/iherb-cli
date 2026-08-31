//! The search path: `parse_search_from_html`, `build_search_url` and
//! `SortOrder::as_url_param`.
//!
//! Several assertions here read the *captured page* for the truth and compare
//! the CLI against it — that is how #3's and #4's bugs become visible without a
//! network call.

use std::collections::BTreeMap;

use iherb_cli::cli::SortOrder;
use iherb_cli::scraper::search::{build_search_url, pages_needed, parse_search_from_html};
use scraper::Selector;

use crate::fixture::{BASE_URL, SEARCH_VITAMIN_C};

// ---------------------------------------------------------------------------
// parse_search_from_html
// ---------------------------------------------------------------------------

#[test]
fn search_page_yields_a_full_page_of_products() {
    let result = parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL, "USD")
        .expect("the search page must parse");

    assert_eq!(result.query, "vitamin c");
    // One result page is 48 cards; the header says "1 - 48 of 11,952 results".
    assert_eq!(result.products.len(), 48);
    assert_eq!(result.total_results, Some(11_952));

    for product in &result.products {
        assert!(!product.name.is_empty());
        assert!(!product.product_id.is_empty());
        assert!(product.price > 0.0, "{}", product.name);
        assert!(product.product_url.starts_with(BASE_URL));
        assert_eq!(product.currency, "USD");
    }
}

#[test]
fn search_cards_carry_brand_rating_and_discount() {
    let result =
        parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL, "USD").unwrap();

    let first = &result.products[0];
    assert_eq!(first.product_id, "61864");
    assert_eq!(first.brand, "California Gold Nutrition");
    assert_eq!(
        first.name,
        "California Gold Nutrition, Gold C®, USP Grade Vitamin C, 1,000 mg, 60 Veggie Capsules"
    );
    assert_eq!(first.price, 5.56);
    assert_eq!(first.original_price, None);
    assert_eq!(first.rating, Some(4.8));
    assert_eq!(first.review_count, Some(381_864));
    assert!(first.in_stock);
    assert_eq!(
        first.product_url,
        "https://www.iherb.com/pr/california-gold-nutrition-gold-c-usp-grade-vitamin-c-1-000-mg-60-veggie-capsules/61864"
    );

    // The strikethrough price is read from the card, not from any JSON.
    let discounted = &result.products[1];
    assert_eq!(discounted.product_id, "61865");
    assert_eq!(discounted.price, 13.11);
    assert_eq!(discounted.original_price, Some(15.42));
}

/// CHARACTERIZATION, NOT DESIRED: pins #33. One page of 48 cards contains only 45
/// distinct products. Three of them are placed twice — sponsored slots repeated
/// in the grid — and the parser returns each card as its own result, so a
/// `--limit 48` search hands the caller three duplicates and three fewer
/// products than it thinks. An agent ranking these counts the same product
/// twice.
///
/// Adjacent to #6's "fewer results than you asked for", but a separate defect.
/// #33 flips this to `seen.len() == result.products.len()`.
#[test]
fn search_cards_repeat_the_same_product() {
    let result =
        parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL, "USD").unwrap();

    let mut seen = BTreeMap::new();
    for product in &result.products {
        *seen.entry(product.product_id.as_str()).or_insert(0) += 1;
    }
    let repeated: Vec<_> = seen
        .iter()
        .filter(|(_, &n)| n > 1)
        .map(|(id, &n)| (*id, n))
        .collect();

    assert_eq!(repeated, vec![("102616", 2), ("82188", 2), ("82189", 2)]);
    assert_eq!(seen.len(), 45);
    assert_eq!(result.products.len(), 48);
}

/// CHARACTERIZATION, NOT DESIRED: the same #5 shape as the product page. The
/// requested currency is discarded whenever the page carries one.
#[test]
fn search_ignores_the_requested_currency() {
    let result =
        parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL, "CHF").unwrap();
    assert!(result.products.iter().all(|p| p.currency == "USD"));
}

#[test]
fn a_page_with_no_cards_is_an_empty_result_not_an_error() {
    let result = parse_search_from_html("<html><body></body></html>", "nothing", BASE_URL, "USD")
        .expect("an empty page is not an error");
    assert!(result.products.is_empty());
    assert_eq!(result.total_results, None);
    assert_eq!(result.query, "nothing");
}

// ---------------------------------------------------------------------------
// build_search_url and SortOrder
// ---------------------------------------------------------------------------

#[test]
fn search_urls_encode_the_query_and_the_page() {
    let first = build_search_url(BASE_URL, "vitamin c", SortOrder::PriceAsc, None, 1);
    assert_eq!(first, "https://www.iherb.com/search?kw=vitamin+c&sr=4");

    let third = build_search_url(BASE_URL, "vitamin c", SortOrder::PriceAsc, None, 3);
    assert_eq!(third, "https://www.iherb.com/search?kw=vitamin+c&sr=4&p=3");

    // Page 1 carries no `p`, so the first URL matches the one a user would land
    // on from the site itself.
    assert!(!first.contains("&p="));

    assert_eq!(pages_needed(1), 1);
    assert_eq!(pages_needed(48), 1);
    assert_eq!(pages_needed(49), 2);
    assert_eq!(pages_needed(200), 5);
}

/// The sort options iHerb's own dropdown offers, read out of the captured
/// search page: `sr` value to label.
fn sort_options_on_the_page() -> BTreeMap<i32, String> {
    let doc = SEARCH_VITAMIN_C.doc();
    let option = Selector::parse("#sort-by-listbox div[role='option']").unwrap();
    let label = Selector::parse("label").unwrap();
    doc.select(&option)
        .filter_map(|el| {
            let val: i32 = el.value().attr("data-val")?.parse().ok()?;
            let text = el.select(&label).next()?.text().collect::<String>();
            Some((val, text.trim().to_string()))
        })
        .collect()
}

/// #3, landed. iHerb's dropdown says Relevance is `sr=0` and Featured is
/// `sr=13`, and `sr=13` is the option the page marks selected — i.e. what you
/// get with no `sr` at all. `--sort relevance` used to emit no `sr` and so
/// returned Featured; it now asks for `sr=0`, and Featured has a variant of its
/// own rather than being reachable only by omission.
#[test]
fn relevance_asks_for_relevance_and_featured_has_its_own_name() {
    let options = sort_options_on_the_page();
    assert_eq!(options.get(&0).map(String::as_str), Some("Relevance"));
    assert_eq!(options.get(&13).map(String::as_str), Some("Featured"));
    // 13 is still the option the page marks selected, so an absent `sr` is
    // still Featured — which is why `relevance` must not be absent.
    assert_eq!(default_sort_on_the_page(), 13);

    assert_eq!(SortOrder::Relevance.as_url_param(), "&sr=0");
    let url = build_search_url(BASE_URL, "vitamin c", SortOrder::Relevance, None, 1);
    assert_eq!(url, "https://www.iherb.com/search?kw=vitamin+c&sr=0");

    assert_eq!(SortOrder::Featured.as_url_param(), "&sr=13");
    assert_eq!(
        build_search_url(BASE_URL, "vitamin c", SortOrder::Featured, None, 1),
        "https://www.iherb.com/search?kw=vitamin+c&sr=13"
    );

    // The two orderings are different requests, which is the whole of #3.
    assert_ne!(
        SortOrder::Relevance.as_url_param(),
        SortOrder::Featured.as_url_param()
    );
}

/// No `--sort` value asks for an ordering by saying nothing. An empty
/// `as_url_param` is how #3 happened: the variant that emitted nothing got the
/// site's default rather than the ordering it was named after, and nothing in
/// the URL recorded which ordering had been asked for.
#[test]
fn every_sort_names_the_ordering_it_wants() {
    for &sort in SortOrder::ALL {
        let param = sort.as_url_param();
        assert_eq!(
            param,
            format!("&sr={}", sort.sr()),
            "{:?} must ask for its ordering by number",
            sort
        );
        assert!(
            build_search_url(BASE_URL, "q", sort, None, 1).contains("&sr="),
            "{:?} produced a URL with no sr",
            sort
        );
    }
}

/// Every `--sort` value, checked against the page's own dropdown rather than
/// against a copy of the mapping written out again.
///
/// Before #3 this covered four variants; the site offers eleven orderings and
/// the CLI now names nine of them.
#[test]
fn every_sort_maps_to_the_value_the_page_uses() {
    let options = sort_options_on_the_page();
    for (sort, sr, label) in [
        (SortOrder::Relevance, 0, "Relevance"),
        (SortOrder::Rating, 1, "Top Rated"),
        (SortOrder::BestSelling, 2, "Best sellers"),
        (SortOrder::PriceDesc, 3, "Price: High to Low"),
        (SortOrder::PriceAsc, 4, "Price: Low to High"),
        (SortOrder::Newest, 10, "Newest"),
        (SortOrder::MostRated, 12, "Most Rated"),
        (SortOrder::Featured, 13, "Featured"),
        (SortOrder::HighestDiscount, 14, "Highest Discount"),
    ] {
        assert_eq!(
            options.get(&sr).map(String::as_str),
            Some(label),
            "the page no longer offers sr={}",
            sr
        );
        assert_eq!(i32::from(sort.sr()), sr, "{:?}", sort);
        assert_eq!(sort.as_url_param(), format!("&sr={}", sr));
    }

    // Every variant is covered above, so the enum cannot grow one that nothing
    // checks against the page.
    assert_eq!(SortOrder::ALL.len(), 9);
}

/// The `sr` value the page treats as the default, i.e. what an absent `sr`
/// gets you: the dropdown option marked `selected`.
fn default_sort_on_the_page() -> i32 {
    let doc = SEARCH_VITAMIN_C.doc();
    let sel = Selector::parse("#sort-by-listbox div[role='option'][aria-selected='true']").unwrap();
    doc.select(&sel)
        .find_map(|el| el.value().attr("data-val")?.parse().ok())
        .expect("the dropdown marks one option selected")
}

/// CHARACTERIZATION, NOT DESIRED — narrowed by #3. Two of the eleven orderings
/// iHerb offers still cannot be produced by any `--sort` value: Heaviest (6)
/// and Lightest (7).
///
/// #3 removed the other four from this list. It pointed `relevance` at `sr=0`,
/// added `most-rated` (12), `newest` (10) and `highest-discount` (14), and gave
/// 13 an explicit `featured` variant so the merchandised order is reachable
/// under its own name rather than by emitting nothing. Heaviest and Lightest
/// are what #3 deliberately did not propose exposing; whoever files that flips
/// this. Do not add sort variants to satisfy this test.
#[test]
fn two_of_the_sites_orderings_cannot_be_produced_at_all() {
    let options = sort_options_on_the_page();
    let reachable: std::collections::BTreeSet<i32> =
        SortOrder::ALL.iter().map(|&s| i32::from(s.sr())).collect();

    // Nine variants, nine distinct orderings: none collides with another.
    assert_eq!(reachable.len(), SortOrder::ALL.len());
    // Featured included, now by name rather than by omission.
    assert!(reachable.contains(&default_sort_on_the_page()));

    let unreachable: Vec<_> = options
        .iter()
        .filter(|(sr, _)| !reachable.contains(sr))
        .map(|(sr, label)| (*sr, label.as_str()))
        .collect();

    assert_eq!(unreachable, vec![(6, "Heaviest"), (7, "Lightest")]);
}

/// Cache keys and URL params are two different vocabularies for the same enum;
/// neither may collide.
#[test]
fn every_sort_has_a_distinct_cache_key() {
    let keys: std::collections::BTreeSet<_> =
        SortOrder::ALL.iter().map(|s| s.as_cache_key()).collect();
    assert_eq!(keys.len(), SortOrder::ALL.len());

    // `relevance` deliberately does not key as "relevance": #3 changed what the
    // variant asks iHerb for, so entries cached under the old identifier hold a
    // different ordering and must not be served for it.
    assert_eq!(SortOrder::Relevance.as_cache_key(), "relevance-sr0");
    assert_eq!(SortOrder::Featured.as_cache_key(), "featured");
}

/// CHARACTERIZATION, NOT DESIRED: pins the #4 bug against the page that proves
/// it. `cids` is a numeric category-id list — every facet link on the captured
/// search page uses one — but `--category` puts its argument in verbatim, so
/// the documented `--category supplements` emits `cids=supplements`.
///
/// #4 flips this: a slug either resolves to an id or the command errors. Do not
/// "fix" `build_search_url` to satisfy this test; fix the test when #4 lands.
#[test]
fn category_slugs_go_into_cids_unresolved() {
    let url = build_search_url(
        BASE_URL,
        "vitamin c",
        SortOrder::Rating,
        Some("supplements"),
        1,
    );
    assert_eq!(
        url,
        "https://www.iherb.com/search?kw=vitamin+c&sr=1&cids=supplements"
    );

    let facets = category_ids_on_the_page();
    assert!(
        !facets.is_empty(),
        "the search page should offer category facets"
    );
    assert!(
        facets
            .iter()
            .all(|id| id.chars().all(|c| c.is_ascii_digit())),
        "every cids the site itself links to is numeric: {:?}",
        facets
    );
    assert!(!facets.contains(&"supplements".to_string()));
}

/// The `cids` values the captured search page links to.
fn category_ids_on_the_page() -> Vec<String> {
    let doc = SEARCH_VITAMIN_C.doc();
    let sel = Selector::parse("label[data-url]").unwrap();
    doc.select(&sel)
        .filter_map(|el| el.value().attr("data-url"))
        .filter_map(|url| url.split("cids=").nth(1))
        .map(|rest| {
            rest.split(['&', '"'])
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}
