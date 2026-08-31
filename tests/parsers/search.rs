//! The search path: `parse_search_from_html`, `build_search_url` and
//! `SortOrder::as_url_param`.
//!
//! Several assertions here read the *captured page* for the truth and compare
//! the CLI against it — that is how #3's and #4's bugs become visible without a
//! network call.

use std::collections::BTreeMap;

use iherb_cli::cli::SortOrder;
use iherb_cli::fetch::{FetchTarget, Paging};
use iherb_cli::scraper::search::{
    build_search_url, pages_needed, parse_search_from_html, CategoryId, CATEGORY_ALIASES,
};
use iherb_cli::targets::search::SearchPages;
use iherb_cli::targets::SearchTarget;
use scraper::Selector;

use crate::fixture::{BASE_URL, CATEGORY_SUPPLEMENTS, SEARCH_VITAMIN_C};

// ---------------------------------------------------------------------------
// parse_search_from_html
// ---------------------------------------------------------------------------

#[test]
fn search_page_yields_a_full_page_of_products() {
    let result = parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL, "USD")
        .expect("the search page must parse");

    assert_eq!(result.query, "vitamin c");
    // One result page is 48 cards and the header says "1 - 48 of 11,952
    // results", but three of those cards repeat a product placed earlier in the
    // grid, so the page is 45 products (#33).
    assert_eq!(result.products.len(), 45);
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

/// #33, landed. One page of 48 cards contains only 45 distinct products: three
/// of them are placed twice — promoted slots repeated in the grid — and the
/// parser used to return each card as its own result, so a `--limit 48` search
/// handed the caller three duplicates and three fewer products than it thought.
/// An agent ranking these counted the same product twice.
#[test]
fn a_product_placed_twice_is_returned_once() {
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

    assert!(repeated.is_empty(), "still repeated: {:?}", repeated);
    assert_eq!(seen.len(), 45);
    assert_eq!(result.products.len(), 45);

    // The three that used to repeat are each present exactly once, by name, so
    // a dedup that quietly dropped them entirely would fail here too.
    for id in ["102616", "82188", "82189"] {
        assert_eq!(
            seen.get(id),
            Some(&1),
            "{} appears {:?} times",
            id,
            seen.get(id)
        );
    }

    // The page really does still carry 48 cards; the parser is what changed.
    let card = Selector::parse("div.product-cell-container").unwrap();
    assert_eq!(SEARCH_VITAMIN_C.doc().select(&card).count(), 48);
}

/// The first placement wins, so the order the caller sees is the order iHerb
/// ranked. Dropping the first copy and keeping a later one would move a product
/// down the list for no reason a caller could see.
#[test]
fn the_first_placement_of_a_repeated_product_is_the_one_kept() {
    let result =
        parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL, "USD").unwrap();

    let card = Selector::parse("div.product-cell-container").unwrap();
    let link = Selector::parse("a.absolute-link.product-link, a.product-link").unwrap();
    let doc = SEARCH_VITAMIN_C.doc();
    let mut first_seen_order = Vec::new();
    for el in doc.select(&card) {
        let Some(id) = el
            .select(&link)
            .next()
            .and_then(|l| l.value().attr("data-product-id"))
        else {
            continue;
        };
        if !first_seen_order.iter().any(|seen| seen == id) {
            first_seen_order.push(id.to_string());
        }
    }

    let parsed: Vec<_> = result
        .products
        .iter()
        .map(|p| p.product_id.clone())
        .collect();
    assert_eq!(parsed, first_seen_order);
}

/// The other half of #33: a product promoted onto one page and listed again on
/// the next is one product. Deduplicating each page in isolation cannot see
/// that, so the accumulator carries the set of ids across pages.
///
/// Driven through `SearchTarget::absorb` — the paging rules `extract` applies
/// to every page production fetches — because the parser only ever sees one
/// page and so cannot be asked this question at all.
#[test]
fn a_product_repeated_across_pages_is_returned_once() {
    let config = search_config();
    let target = SearchTarget::new(&config, "vitamin c", 200, SortOrder::Relevance, None).unwrap();

    // The same captured page handed to the target twice: every product on the
    // second page is one the first already yielded, so it must add none.
    let mut acc = SearchPages::default();
    assert_eq!(target.absorb(one_captured_page(), &mut acc), Paging::More);
    assert_eq!(acc.gathered(), 45);
    assert_eq!(target.absorb(one_captured_page(), &mut acc), Paging::More);
    assert_eq!(
        acc.gathered(),
        45,
        "a second page of the same products must add none"
    );

    let result = target.finish(acc).unwrap();
    assert_eq!(result.products.len(), 45);
    let distinct: std::collections::BTreeSet<_> = result
        .products
        .iter()
        .map(|p| p.product_id.as_str())
        .collect();
    assert_eq!(distinct.len(), 45);
}

/// The captured results page, parsed exactly as `extract` parses each page it
/// navigates to.
fn one_captured_page() -> iherb_cli::model::SearchResult {
    parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL, "USD").unwrap()
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

/// #4, landed. `cids` is a numeric category-id list — every facet link on the
/// captured search page uses one — and `--category` used to put its argument in
/// verbatim, so the documented `--category supplements` emitted
/// `cids=supplements`, which iHerb ignores. The search returned everything and
/// the caller believed it had filtered.
///
/// A slug now resolves to an id or the command fails; nothing else reaches the
/// URL, because [`CategoryId`] is the only thing `build_search_url` accepts.
#[test]
fn a_category_slug_resolves_to_the_id_cids_expects() {
    let supplements = CategoryId::resolve("supplements").expect("a documented slug must resolve");
    let url = build_search_url(
        BASE_URL,
        "vitamin c",
        SortOrder::Rating,
        Some(&supplements),
        1,
    );
    assert_eq!(
        url,
        "https://www.iherb.com/search?kw=vitamin+c&sr=1&cids=1855"
    );

    // A numeric id is a category too: the site's own facet links carry those,
    // and an id with no name in the table still has to work.
    let professional_brands = CategoryId::resolve("107703").unwrap();
    assert_eq!(professional_brands.as_str(), "107703");
    assert!(build_search_url(
        BASE_URL,
        "vitamin c",
        SortOrder::Rating,
        Some(&professional_brands),
        1
    )
    .ends_with("&cids=107703"));

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

/// An unresolvable `--category` is an error, not a silent no-op. That is the
/// half of #4 that matters most: a filter that cannot be honoured must say so,
/// because a caller cannot tell a search that ignored its filter from one that
/// honoured it and found everything anyway.
#[test]
fn an_unresolvable_category_is_refused() {
    for input in ["not-a-category", "", "   ", "1855abc", "supplements!"] {
        let err = CategoryId::resolve(input)
            .expect_err(&format!("{:?} must not resolve", input))
            .to_string();
        assert!(err.contains("Unknown --category"), "{}", err);
        // The message has to be usable: it names what does work.
        assert!(err.contains("supplements"), "{}", err);
    }

    // And it fails at the target, before anything launches a browser.
    let config = search_config();
    assert!(SearchTarget::new(
        &config,
        "vitamin c",
        20,
        SortOrder::Relevance,
        Some("not-a-category")
    )
    .is_err());
}

/// Both spellings of the same category are the same request, so they are the
/// same cache entry rather than two copies of one fetch.
#[test]
fn a_slug_and_its_id_are_the_same_request() {
    let config = search_config();
    let by_slug = SearchTarget::new(
        &config,
        "vitamin c",
        20,
        SortOrder::Rating,
        Some("supplements"),
    )
    .unwrap();
    let by_id =
        SearchTarget::new(&config, "vitamin c", 20, SortOrder::Rating, Some("1855")).unwrap();

    assert_eq!(by_slug.url(1), by_id.url(1));
    assert!(by_slug.url(1).contains("&cids=1855"));
    assert_eq!(
        by_slug.cache_key().file_name(),
        by_id.cache_key().file_name()
    );

    // A different category is a different entry.
    let other =
        SearchTarget::new(&config, "vitamin c", 20, SortOrder::Rating, Some("herbs")).unwrap();
    assert_ne!(
        by_slug.cache_key().file_name(),
        other.cache_key().file_name()
    );
}

/// Every id in the alias table was read off a captured page.
///
/// This is what stops the table becoming a list of plausible numbers. Each row
/// has to appear in one of the two captures' category facets, under a title the
/// slug is derived from — so a row nobody can point at a page for fails here,
/// and a slug quietly repointed at a different id fails too.
#[test]
fn every_category_alias_is_a_category_the_captured_pages_name() {
    let named = categories_named_by_the_captured_pages();

    for (slug, id) in CATEGORY_ALIASES {
        let title = named
            .get(*id)
            .unwrap_or_else(|| panic!("no captured page names category {} ({})", id, slug));
        assert_eq!(
            &slugify(title),
            slug,
            "category {} is titled {:?} on the page",
            id,
            title
        );
    }

    // No two names for one id, and no id under two names.
    let slugs: std::collections::BTreeSet<_> = CATEGORY_ALIASES.iter().map(|(s, _)| *s).collect();
    let ids: std::collections::BTreeSet<_> = CATEGORY_ALIASES.iter().map(|(_, i)| *i).collect();
    assert_eq!(slugs.len(), CATEGORY_ALIASES.len());
    assert_eq!(ids.len(), CATEGORY_ALIASES.len());
}

/// `mushrooms` is the one name the captures disagree about: the nav links
/// `/c/mushrooms?cids=101022`, the category facet titles 100945 "Mushrooms".
/// Nothing says which one `--category mushrooms` should mean, so it resolves to
/// neither — and both ids still work as ids.
#[test]
fn the_ambiguous_name_is_left_unresolved() {
    assert!(CategoryId::resolve("mushrooms").is_err());
    assert_eq!(CategoryId::resolve("101022").unwrap().as_str(), "101022");
    assert_eq!(CategoryId::resolve("100945").unwrap().as_str(), "100945");

    assert!(SEARCH_VITAMIN_C.html().contains("/c/mushrooms?cids=101022"));
    assert_eq!(
        categories_named_by_the_captured_pages()
            .get("100945")
            .map(String::as_str),
        Some("Mushrooms")
    );
}

/// A config for the target-level assertions above. `SearchTarget` needs one and
/// nothing here touches the cache directory it names.
fn search_config() -> iherb_cli::config::AppConfig {
    iherb_cli::config::AppConfig {
        country: "us".to_string(),
        currency: "USD".to_string(),
        no_cache: false,
        delay_ms: 0,
        debug: false,
        browser_path: None,
        cache_dir: std::path::PathBuf::from("/nonexistent"),
        data_dir: std::path::PathBuf::from("/nonexistent"),
    }
}

/// Category id to title, from the category facet on every captured page that
/// has one. This is where the alias table's ids come from.
fn categories_named_by_the_captured_pages() -> BTreeMap<String, String> {
    let sel = Selector::parse("[data-category-id][title]").unwrap();
    let mut out = BTreeMap::new();
    for fixture in [SEARCH_VITAMIN_C, CATEGORY_SUPPLEMENTS] {
        let doc = fixture.doc();
        for el in doc.select(&sel) {
            let (Some(id), Some(title)) = (
                el.value().attr("data-category-id"),
                el.value().attr("title"),
            ) else {
                continue;
            };
            out.insert(id.to_string(), title.to_string());
        }
    }
    out
}

/// The slug rule the alias table follows: drop any parenthesised gloss, drop
/// apostrophes without leaving a gap, lowercase, and hyphenate everything else
/// that is not a letter or a digit.
///
/// The two exceptions earn their place. An apostrophe that leaves a gap turns
/// "Children's Health" into `children-s-health`, and the gloss in "Omegas &
/// Fish Oils (EPA DHA)" is an explanation rather than part of the name.
fn slugify(title: &str) -> String {
    let without_gloss: String = {
        let mut out = String::new();
        let mut depth = 0usize;
        for ch in title.chars() {
            match ch {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ if depth == 0 => out.push(ch),
                _ => {}
            }
        }
        out
    };

    let mut out = String::new();
    let mut pending_gap = false;
    for ch in without_gloss.chars() {
        match ch {
            // Dropped outright, so a possessive does not become its own word.
            '\'' | '\u{2019}' => {}
            c if c.is_ascii_alphanumeric() => {
                if pending_gap && !out.is_empty() {
                    out.push('-');
                }
                pending_gap = false;
                out.push(c.to_ascii_lowercase());
            }
            _ => pending_gap = true,
        }
    }
    out
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
