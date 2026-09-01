//! The search path: `parse_search_from_html`, `build_search_url` and
//! `SortOrder::as_url_param`.
//!
//! Several assertions here read the *captured page* for the truth and compare
//! the CLI against it — that is how #3's and #4's bugs become visible without a
//! network call.

use std::collections::BTreeMap;

use iherb_cli::cache::Cache;
use iherb_cli::cli::SortOrder;
use iherb_cli::fetch::{cached, FetchTarget, Paging};
use iherb_cli::model::{Source, Strategy};
use iherb_cli::output::format_search_results;
use iherb_cli::scraper::search::{
    build_search_url, page_budget, parse_search_from_html, CategoryId, CATEGORY_ALIASES,
};
use iherb_cli::targets::search::SearchPages;
use iherb_cli::targets::SearchTarget;
use scraper::Selector;

use crate::fixture::TempDir;
use crate::fixture::{BASE_URL, CATEGORY_SUPPLEMENTS, SEARCH_VITAMIN_C};

// ---------------------------------------------------------------------------
// parse_search_from_html
// ---------------------------------------------------------------------------

#[test]
fn search_page_yields_a_full_page_of_products() {
    let result = parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL)
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
        assert!(
            product.price.is_some_and(|p| p > 0.0),
            "{}: {:?}",
            product.name,
            product.price
        );
        assert!(product.product_url.starts_with(BASE_URL));
        assert_eq!(product.currency.as_deref(), Some("USD"));
    }
}

#[test]
fn search_cards_carry_brand_rating_and_discount() {
    let result = parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL).unwrap();

    let first = &result.products[0];
    assert_eq!(first.product_id, "61864");
    assert_eq!(first.brand, "California Gold Nutrition");
    assert_eq!(
        first.name,
        "California Gold Nutrition, Gold C®, USP Grade Vitamin C, 1,000 mg, 60 Veggie Capsules"
    );
    assert_eq!(first.price, Some(5.56));
    assert_eq!(first.original_price, None);
    assert_eq!(first.rating, Some(4.8));
    assert_eq!(first.review_count, Some(381_864));
    assert_eq!(first.in_stock, Some(true));
    assert_eq!(
        first.product_url,
        "https://www.iherb.com/pr/california-gold-nutrition-gold-c-usp-grade-vitamin-c-1-000-mg-60-veggie-capsules/61864"
    );

    // The strikethrough price is read from the card, not from any JSON.
    let discounted = &result.products[1];
    assert_eq!(discounted.product_id, "61865");
    assert_eq!(discounted.price, Some(13.11));
    assert_eq!(discounted.original_price, Some(15.42));
}

/// #33, landed. One page of 48 cards contains only 45 distinct products: three
/// of them are placed twice — promoted slots repeated in the grid — and the
/// parser used to return each card as its own result, so a `--limit 48` search
/// handed the caller three duplicates and three fewer products than it thought.
/// An agent ranking these counted the same product twice.
#[test]
fn a_product_placed_twice_is_returned_once() {
    let result = parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL).unwrap();

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
    let result = parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL).unwrap();

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
    parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL).unwrap()
}

/// FLIPPED BY #5. This was the search half of the characterization: the parser
/// took a `currency` argument, threw it away whenever the page carried a marker
/// of its own, and stamped it onto every card when the page did not.
///
/// The argument is gone. Every card on a results page carries the one currency
/// the page published, because iHerb publishes one for the whole page.
#[test]
fn every_card_carries_the_currency_the_page_published() {
    let result = parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL).unwrap();
    assert!(result
        .products
        .iter()
        .all(|p| p.currency.as_deref() == Some("USD")));
    assert!(result
        .products
        .iter()
        .all(|p| p.source_of("currency") == Source::Dom));
}

#[test]
fn a_page_with_no_cards_is_an_empty_result_not_an_error() {
    let result = parse_search_from_html("<html><body></body></html>", "nothing", BASE_URL)
        .expect("an empty page is not an error");
    assert!(result.products.is_empty());
    assert_eq!(result.total_results, None);
    assert_eq!(result.query, "nothing");
}

// ---------------------------------------------------------------------------
// #49 — what the search path read, and what it did not
// ---------------------------------------------------------------------------

/// Every value on a search card names the strategy that produced it, exactly as
/// a product page's values do (#28, extended by #49).
///
/// Not one value is checked here; every assertion is about where a value came
/// from. A card that stopped carrying `data-ga-brand-name` and started getting
/// its brand from somewhere else would pass every value assertion in this file
/// and fail this one.
#[test]
fn every_card_field_names_the_strategy_that_produced_it() {
    let result = parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL)
        .expect("the search page must parse");
    let first = &result.products[0];

    // One strategy reads a results page: CSS selectors over its HTML.
    assert_eq!(first.extraction.strategy, Strategy::Dom);
    for field in [
        "name",
        "brand",
        "price",
        "currency",
        "rating",
        "review_count",
        "product_url",
        "product_id",
        "in_stock",
    ] {
        assert_eq!(first.source_of(field), Source::Dom, "{}", field);
    }

    // The first card is not discounted, so there is no original price — absent,
    // which is a different answer from a price that failed to parse.
    assert_eq!(first.original_price, None);
    assert_eq!(first.source_of("original_price"), Source::Absent);

    // A discounted card has one, and it was read.
    let discounted = &result.products[1];
    assert!(discounted.original_price.is_some());
    assert_eq!(discounted.source_of("original_price"), Source::Dom);

    // No card on this page is degraded: the capture publishes everything a
    // results card is expected to carry.
    for product in &result.products {
        assert!(!product.health().degraded, "{}", product.product_id);
    }
}

/// A currency the page did not publish is not attributed to the page.
///
/// `search.rs` used to stamp `detect_currency_from_html(..).unwrap_or(currency)`
/// on every card, so the `--currency` label — a string from the command line —
/// was presented exactly as a currency read off the page. That is #49's first
/// fabrication, and it is the same shape #28 removed from the product path.
///
/// #49 stopped the label being vouched for; #5 stopped it existing. The
/// assertions below moved with it: a card whose page published no currency used
/// to hold the caller's label as [`Source::Defaulted`], and now holds nothing
/// as [`Source::Absent`]. Both states are degraded, which is the part #49 built
/// and #5 keeps.
#[test]
fn an_undetected_currency_is_not_attributed_to_the_page() {
    // The captured page publishes one, so it is read.
    let read = parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL).unwrap();
    assert_eq!(read.products[0].currency.as_deref(), Some("USD"));
    assert_eq!(read.products[0].source_of("currency"), Source::Dom);

    // A page with cards and no currency marker anywhere. There is no longer a
    // label to substitute, so the card has no currency at all.
    let unmarked = parse_search_from_html(&a_card_with_no_currency(), "q", BASE_URL).unwrap();
    let card = &unmarked.products[0];
    assert_eq!(card.currency, None, "there is no label left to substitute");
    assert_eq!(card.source_of("currency"), Source::Absent);

    // And it is visible as such rather than buried: `currency` is a field every
    // results card publishes, so a page that does not publish one is degraded.
    let health = card.health();
    assert!(health.fields_absent.contains(&"currency".to_string()));
    assert!(
        health.degraded,
        "a card with no currency must be able to make a record degraded"
    );
}

/// A card whose price neither source could parse has no price.
///
/// `unwrap_or(0.0)` made three different situations one value: a genuinely free
/// product, a card whose price markup changed, and a card that carries no price
/// at all. `$0.00` printed for any of them, and a caller sorting by price put
/// them first.
#[test]
fn a_card_with_no_readable_price_has_none() {
    let result = parse_search_from_html(&a_card_with_no_price(), "q", BASE_URL).unwrap();
    let card = &result.products[0];

    assert_eq!(card.price, None);
    assert_eq!(card.source_of("price"), Source::Absent);
    assert!(card.health().fields_absent.contains(&"price".to_string()));
    assert!(
        card.health().degraded,
        "price is a field every card publishes"
    );

    // And the rendering says so instead of showing a free product.
    let rendered = format_search_results(&result);
    assert!(rendered.contains("no price could be read"), "{}", rendered);
    assert!(!rendered.contains("$0.00"), "{}", rendered);

    // A strikethrough price with no price to compare it against is not a
    // discount, and is dropped rather than presented as one.
    assert_eq!(card.original_price, None);
}

/// A card that says nothing about stock says nothing, rather than "yes".
///
/// The search path's `unwrap_or(true)` is #31's bug: a card whose markup we no
/// longer understand — or that simply omits the attribute — was reported as
/// purchasable. `ProductDetail::in_stock` became an `Option` for this reason;
/// `ProductSummary::in_stock` is one now for the same reason, which is also
/// what lets #9 render the field the same way in both commands.
#[test]
fn a_card_with_no_stock_signal_does_not_claim_to_be_in_stock() {
    let result = parse_search_from_html(&a_card_with_no_stock_marker(), "q", BASE_URL).unwrap();
    let card = &result.products[0];

    assert_eq!(card.in_stock, None);
    assert_eq!(card.source_of("in_stock"), Source::Absent);

    // Out of stock is still read as out of stock, so this is not the parser
    // giving up on the field.
    let out = parse_search_from_html(&a_card_out_of_stock(), "q", BASE_URL).unwrap();
    assert_eq!(out.products[0].in_stock, Some(false));
    assert_eq!(out.products[0].source_of("in_stock"), Source::Dom);

    // And every card on the real capture carries the attribute, so this is not
    // a change that quietly blanks the field on live pages.
    let real = parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL).unwrap();
    assert!(real.products.iter().all(|p| p.in_stock == Some(true)));
}

/// #49's last acceptance criterion: one shape, not two.
///
/// A search result and a product detail report on themselves through the same
/// type, with the same keys and the same vocabulary, so #9 renders one
/// provenance block under `--json` rather than one that exists on products and
/// silently not on search.
#[test]
fn a_search_card_reports_its_health_in_the_same_shape_a_product_does() {
    let result = parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL).unwrap();
    let json = serde_json::to_value(result.products[0].health()).expect("must serialize");

    // The same keys `provenance::health_serializes_to_the_block_issue_9_renders`
    // pins for a product.
    for key in [
        "strategy",
        "enriched",
        "sources",
        "fields_absent",
        "fields_defaulted",
        "degraded",
    ] {
        assert!(json.get(key).is_some(), "missing {}", key);
    }
    assert_eq!(json["strategy"], "dom");
    assert_eq!(json["degraded"], false);
    assert_eq!(json["sources"]["name"], "dom");
    assert_eq!(json["sources"]["original_price"], "absent");

    // And the vocabulary is the same one: `absent` on a card means what it means
    // on a product. It was `defaulted` here until #5, because the card carried
    // the `--currency` label; the label is gone, so what is left is the absence
    // the product path reports for the same page.
    let unmarked = parse_search_from_html(&a_card_with_no_currency(), "q", BASE_URL).unwrap();
    let json = serde_json::to_value(unmarked.products[0].health()).unwrap();
    assert_eq!(json["sources"]["currency"], "absent");
    assert_eq!(json["degraded"], true);
}

/// A cached search written before #49 still loads, and comes back honest about
/// knowing nothing — rather than failing to parse, or claiming the DOM produced
/// values nobody recorded.
#[test]
fn a_pre_provenance_search_entry_still_deserializes() {
    let stored = serde_json::json!({
        "query": "vitamin c",
        "total_results": 11_952,
        "products": [{
            "name": "Acme, Vitamin C, 60 Capsules",
            "brand": "Acme",
            "price": 12.34,
            "original_price": null,
            "currency": "USD",
            "rating": 4.5,
            "review_count": 7,
            "product_url": "https://www.iherb.com/pr/acme/1",
            "product_id": "1",
            "in_stock": true,
        }],
    });

    let result: iherb_cli::model::SearchResult =
        serde_json::from_value(stored).expect("an old entry must still load");
    let card = &result.products[0];
    assert_eq!(card.price, Some(12.34));
    assert_eq!(card.in_stock, Some(true));
    assert_eq!(card.extraction.strategy, Strategy::Unrecorded);
    assert_eq!(card.source_of("name"), Source::Absent);
    assert_eq!(result.fetch, iherb_cli::model::SearchFetch::default());
}

/// A results card, with only the parts a test cares about filled in. The
/// selectors are the ones `parse_product_card` reads, so a card built here goes
/// down the same path a real one does.
fn a_card(price_markup: &str, stock_attr: &str, currency_marker: &str) -> String {
    format!(
        r#"<html><body>{currency_marker}
        <div class="product-cell-container">
          <div class="product ga-product" {stock_attr}>
            <a class="absolute-link product-link" href="/pr/thing/1"
               data-product-id="1" data-ga-brand-name="Acme" title="Acme, Thing"></a>
            <div class="product-title" content="Acme, Thing"></div>
            {price_markup}
          </div>
        </div></body></html>"#
    )
}

/// A card with a price, on a page whose currency cannot be detected: no
/// `priceCurrency` meta, and a price with no recognisable symbol.
fn a_card_with_no_currency() -> String {
    a_card(
        r#"<meta itemprop="price" content="9.60">"#,
        r#"data-is-out-of-stock="false""#,
        "",
    )
}

/// A card whose price neither the microdata nor the link attributes carry.
fn a_card_with_no_price() -> String {
    a_card(
        "",
        r#"data-is-out-of-stock="false""#,
        r#"<meta itemprop="priceCurrency" content="USD">"#,
    )
}

/// A card with neither of the two attributes that say whether it is in stock.
fn a_card_with_no_stock_marker() -> String {
    a_card(
        r#"<meta itemprop="price" content="9.60">"#,
        "",
        r#"<meta itemprop="priceCurrency" content="USD">"#,
    )
}

/// A card that says it is out of stock.
fn a_card_out_of_stock() -> String {
    a_card(
        r#"<meta itemprop="price" content="9.60">"#,
        r#"data-is-out-of-stock="true""#,
        r#"<meta itemprop="priceCurrency" content="USD">"#,
    )
}

// ---------------------------------------------------------------------------
// #6 — a cached search that is short of --limit
// ---------------------------------------------------------------------------

/// #6, landed, at the layer that decides it.
///
/// A `--limit 10` run caches the products one page yielded. The later
/// `--limit 200` run used to read that entry, be handed 45, and print a header
/// still quoting "of 11,952" — silently short, with a plausible timestamp and
/// no way for the caller to tell except by counting.
///
/// The entry now records what its walk did, and the search path asks whether
/// what is stored answers the request. It does not, so this reads as a miss and
/// the pipeline refetches. `cached` is the real production path: `fetch` is
/// this plus a browser launch on `None`.
#[test]
fn a_cached_search_short_of_the_limit_is_refetched() {
    let dir = TempDir::new("limit-refetch");
    let config = cache_config(dir.path());
    let cache = Cache::new(
        dir.path(),
        iherb_cli::config::CacheMode::ReadWrite,
        iherb_cli::config::DEFAULT_CACHE_TTL,
    );

    let narrow = SearchTarget::new(&config, "vitamin c", 10, SortOrder::Relevance, None).unwrap();
    cache
        .set(&narrow.cache_key(), &one_page_walked())
        .expect("write the narrow entry");

    // The run that wrote it asked for 10 and got 45, so it is answered.
    let hit = cached(&narrow, &config).expect("45 products answer a request for 10");
    assert_eq!(hit.data.products.len(), 45);

    // A wider request is not: 45 is short of 200, and the entry says the walk
    // stopped with iHerb nowhere near out of results.
    let wide = SearchTarget::new(&config, "vitamin c", 200, SortOrder::Relevance, None).unwrap();
    assert!(
        cached(&wide, &config).is_none(),
        "a request for 200 must not be answered out of a 45-product entry"
    );

    // The entry is still on disk and still the same one: this is a refetch
    // decision, not a second cache file (#1 is what changes the key).
    assert_eq!(dir.file_count(), 1);
    assert_eq!(narrow.cache_key().file_name(), wide.cache_key().file_name());
}

/// The shortfall that is not a bug: iHerb genuinely has no more. A walk that
/// ended because the results ran out answers any limit, however large — asking
/// again would fetch the same pages and find the same products.
#[test]
fn a_cached_search_that_exhausted_the_results_answers_any_limit() {
    let dir = TempDir::new("limit-exhausted");
    let config = cache_config(dir.path());
    let cache = Cache::new(
        dir.path(),
        iherb_cli::config::CacheMode::ReadWrite,
        iherb_cli::config::DEFAULT_CACHE_TTL,
    );

    let mut all_there_is = one_page_walked();
    all_there_is.total_results = Some(45);
    all_there_is.fetch.exhausted = Some(true);

    let target =
        SearchTarget::new(&config, "vitamin c", 1_000, SortOrder::Relevance, None).unwrap();
    cache.set(&target.cache_key(), &all_there_is).unwrap();

    let hit = cached(&target, &config).expect("there is no more to fetch");
    assert_eq!(hit.data.products.len(), 45);
}

/// An entry written before #6 says nothing about its walk. Nothing is not
/// "complete": treating it as complete is exactly the assumption that made #6
/// silent, so a short one is refetched.
#[test]
fn an_entry_that_does_not_say_how_it_was_fetched_is_not_assumed_complete() {
    let dir = TempDir::new("limit-unrecorded");
    let config = cache_config(dir.path());
    let cache = Cache::new(
        dir.path(),
        iherb_cli::config::CacheMode::ReadWrite,
        iherb_cli::config::DEFAULT_CACHE_TTL,
    );

    let mut old_entry = one_page_walked();
    old_entry.fetch = Default::default();
    assert_eq!(old_entry.fetch.pages_fetched, None);
    assert_eq!(old_entry.fetch.exhausted, None);

    let target = SearchTarget::new(&config, "vitamin c", 200, SortOrder::Relevance, None).unwrap();
    cache.set(&target.cache_key(), &old_entry).unwrap();

    assert!(cached(&target, &config).is_none());
}

/// A walk records the pages it took and whether it reached the end, so what is
/// cached carries the facts the decision above is made from.
#[test]
fn a_walk_records_what_it_did() {
    let config = search_config();
    let target = SearchTarget::new(&config, "vitamin c", 200, SortOrder::Relevance, None).unwrap();

    // Two pages of results, then a page with nothing on it: iHerb ran out.
    let mut acc = SearchPages::default();
    assert_eq!(target.absorb(one_captured_page(), &mut acc), Paging::More);
    assert_eq!(target.absorb(one_captured_page(), &mut acc), Paging::More);
    assert_eq!(target.absorb(an_empty_page(), &mut acc), Paging::Done);
    assert_eq!(acc.pages_fetched(), 3);
    assert_eq!(acc.gathered(), 45);

    let result = target.finish(acc).unwrap();
    assert_eq!(result.fetch.pages_fetched, Some(3));
    assert_eq!(result.fetch.exhausted, Some(true));

    // A walk that stops with products still coming says so.
    let mut acc = SearchPages::default();
    target.absorb(one_captured_page(), &mut acc);
    let result = target.finish(acc).unwrap();
    assert_eq!(result.fetch.pages_fetched, Some(1));
    assert_eq!(result.fetch.exhausted, Some(false));
}

/// The captured page, with a walk recorded on it as `SearchTarget::finish`
/// would: one page, and iHerb nowhere near out of results.
fn one_page_walked() -> iherb_cli::model::SearchResult {
    let mut result = one_captured_page();
    result.fetch = iherb_cli::model::SearchFetch {
        pages_fetched: Some(1),
        exhausted: Some(false),
    };
    result
}

/// `--currency` on the search path (#5): a requirement on the storefront,
/// checked against every card, enforced on the fresh path and on the cached one.
///
/// The cards on the captured page are USD, so a run that asked for CHF is
/// refused rather than handed 45 US prices to caption as Swiss ones — which is
/// what the flag did before, whenever currency detection failed.
#[test]
fn a_search_in_the_wrong_currency_is_refused_rather_than_relabelled() {
    let dir = TempDir::new("currency-search");
    let usd_config = iherb_cli::config::AppConfig {
        currency: Some("USD".to_string()),
        ..cache_config(dir.path())
    };
    let chf_config = iherb_cli::config::AppConfig {
        currency: Some("CHF".to_string()),
        ..cache_config(dir.path())
    };

    let walked = one_page_walked();
    assert_eq!(walked.products[0].currency.as_deref(), Some("USD"));

    let asked_usd =
        SearchTarget::new(&usd_config, "vitamin c", 10, SortOrder::Relevance, None).unwrap();
    let asked_chf =
        SearchTarget::new(&chf_config, "vitamin c", 10, SortOrder::Relevance, None).unwrap();
    let unasked = SearchTarget::new(
        &cache_config(dir.path()),
        "vitamin c",
        10,
        SortOrder::Relevance,
        None,
    )
    .unwrap();

    // The fresh path.
    assert!(unasked.validate(&walked).is_ok());
    assert!(asked_usd.validate(&walked).is_ok());
    assert!(
        asked_chf.validate(&walked).is_err(),
        "a USD results page must not satisfy --currency CHF"
    );

    // And the cached path, which `validate` never sees. Two defences now, and
    // the test wants both.
    //
    // The first is the key: since the cookie made `--currency` change the
    // document, a CHF request and a USD request are different entries, so the
    // CHF run cannot reach the USD run's file at all.
    let cache = Cache::new(
        dir.path(),
        iherb_cli::config::CacheMode::ReadWrite,
        iherb_cli::config::DEFAULT_CACHE_TTL,
    );
    cache.set(&asked_usd.cache_key(), &walked).unwrap();
    assert_ne!(
        asked_usd.cache_key().file_name(),
        asked_chf.cache_key().file_name()
    );
    assert!(
        cached(&asked_chf, &chf_config).is_none(),
        "a cached USD entry was served to a --currency CHF request"
    );
    assert!(
        cached(&asked_usd, &usd_config).is_some(),
        "...while the request the entry does answer still hits it"
    );

    // The second is `cache_is_sufficient`, which reads the entry rather than
    // its name. It is behind the key rather than in front of it now, and it
    // still has to hold: an entry whose contents disagree with the request it
    // is filed under is not an answer, however it came to be there.
    cache.set(&asked_chf.cache_key(), &walked).unwrap();
    assert!(
        cached(&asked_chf, &chf_config).is_none(),
        "an entry holding USD prices answered a CHF request because its name matched"
    );

    // A card whose page published no currency confirms nothing, so it cannot
    // satisfy a request either.
    let mut unmarked = walked.clone();
    for product in &mut unmarked.products {
        product.currency = None;
    }
    assert!(asked_usd.validate(&unmarked).is_err());
    assert!(unasked.validate(&unmarked).is_ok());
}

/// What the parser returns for a results page past the end of the results.
fn an_empty_page() -> iherb_cli::model::SearchResult {
    parse_search_from_html("<html><body></body></html>", "vitamin c", BASE_URL).unwrap()
}

/// A config pointed at a scratch cache directory, for the round-trips above.
fn cache_config(cache_dir: std::path::PathBuf) -> iherb_cli::config::AppConfig {
    iherb_cli::config::AppConfig {
        cache_dir,
        ..search_config()
    }
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

    // A page of 48 cards is fewer than 48 products, so the budget carries one
    // page of slack; `has_enough` is what actually stops the walk (#6, #33).
    assert_eq!(page_budget(1), 2);
    assert_eq!(page_budget(48), 2);
    assert_eq!(page_budget(49), 3);
    assert_eq!(page_budget(200), 6);
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
        // No `--currency`, so nothing is required of the storefront (#5).
        currency: None,
        cache_mode: iherb_cli::config::CacheMode::ReadWrite,
        cache_ttl: iherb_cli::config::DEFAULT_CACHE_TTL,
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
