//! #11: the readiness selectors match the pages they claim to.
//!
//! A readiness selector set is a claim about the shape of a real iHerb page,
//! and a claim nothing checks is how a wait quietly becomes an eight-second
//! sleep: `wait_for_selectors` gives up rather than failing, on purpose, so a
//! selector that stopped matching would cost every navigation the whole budget
//! and say so only in a warning nobody reads.
//!
//! So the selectors are run against the captured pages in `tests/fixtures/`
//! with the same `scraper` parser the extractors use. Twenty real pages,
//! matched by the real selector strings taken off `ReadinessTarget` rather than
//! retyped here — a copy in the test would pass while production waited out
//! every page.

use scraper::Selector;

use iherb_cli::scraper::navigation::ReadinessTarget;

use crate::fixture::{self, Fixture};

/// Every selector in a set, parsed. A selector this crate cannot parse is one
/// Chrome would reject too, and `find_element` would answer `Err` for ever.
fn parsed(target: ReadinessTarget) -> Vec<(&'static str, Selector)> {
    let selectors = target.selectors();
    assert!(
        !selectors.is_empty(),
        "{:?} claims a selector set and has none, so every page under it waits \
         out the readiness budget",
        target
    );
    selectors
        .iter()
        .map(|s| {
            (
                *s,
                Selector::parse(s).unwrap_or_else(|e| {
                    panic!(
                        "{:?} carries a selector Chrome could not use, {}: {:?}",
                        target, s, e
                    )
                }),
            )
        })
        .collect()
}

/// The selectors from `target` that are present on `page`.
fn matching(target: ReadinessTarget, page: Fixture) -> Vec<&'static str> {
    let doc = page.doc();
    parsed(target)
        .into_iter()
        .filter(|(_, selector)| doc.select(selector).next().is_some())
        .map(|(name, _)| name)
        .collect()
}

/// **Every captured product page answers the product readiness set.**
///
/// Not "the set parses" and not "one page matches": all twenty, because the set
/// is what every product fetch waits on and a page that matches none of it pays
/// the full eight seconds and then reads whatever is there.
#[test]
fn every_captured_product_page_matches_the_product_readiness_set() {
    for page in fixture::products() {
        let matched = matching(ReadinessTarget::Product, page);
        assert!(
            !matched.is_empty(),
            "no Product readiness selector matches {}, so a fetch of it would \
             wait out the whole budget before reading the page; the set is {:?}",
            page.slug(),
            ReadinessTarget::Product.selectors()
        );
    }
}

/// **Every selector in the product set is real**, not just enough of them.
///
/// A set whose useful members are one selector and three typos passes the test
/// above. Each selector is asserted to match at least one captured page, so a
/// dead one is visible rather than carried.
#[test]
fn every_product_readiness_selector_matches_some_captured_page() {
    for (name, selector) in parsed(ReadinessTarget::Product) {
        let hits = fixture::products()
            .filter(|page| page.doc().select(&selector).next().is_some())
            .count();
        assert!(
            hits > 0,
            "the Product readiness selector {} matches none of the captured \
             product pages, so it can only ever cost time",
            name
        );
    }
}

/// **The search set matches the captured results pages**, and the two that are
/// checkable are checked one by one.
///
/// `.no-results` is deliberately absent from this assertion: this repository
/// holds no capture of an empty result set, so the selector is carried from the
/// fork unverified. That is recorded on `ReadinessTarget::selectors` too. It is
/// safe to carry — a selector that never matches costs the budget and decides
/// nothing — and it is the only thing that would make an empty search return at
/// once, so it is worth carrying.
#[test]
fn the_search_readiness_set_matches_the_captured_results_pages() {
    let pages = [fixture::SEARCH_VITAMIN_C, fixture::CATEGORY_SUPPLEMENTS];

    for page in pages {
        let matched = matching(ReadinessTarget::Search, page);
        assert!(
            matched.contains(&"div.product-cell-container"),
            "the result cards selector does not match {}; matched {:?}",
            page.slug(),
            matched
        );
        assert!(
            matched.contains(&"#product-count"),
            "the result count selector does not match {}; matched {:?}",
            page.slug(),
            matched
        );
    }
}

/// **A product page does not answer the search set, and a results page does not
/// answer the product set.**
///
/// Without this the two sets could be one permissive set that matches
/// everything, which would wait for the wrong evidence on both — ready as soon
/// as any page at all had loaded, which is what `document.readyState` already
/// was and what #11 is replacing.
#[test]
fn the_two_readiness_sets_are_not_interchangeable() {
    let product = fixture::ULTIMATE_OMEGA_NOK;
    assert!(
        !matching(ReadinessTarget::Search, product).contains(&"#product-count"),
        "a product page answers the search result count, so the two sets do not \
         distinguish the pages they are for"
    );

    let results = fixture::SEARCH_VITAMIN_C;
    let on_results = matching(ReadinessTarget::Product, results);
    assert!(
        !on_results.contains(&"h1#name") && !on_results.contains(&"#product-specs-list"),
        "a results page answers the product page's own selectors: {:?}",
        on_results
    );
}

/// **`DocumentComplete` has no selectors, and that is the point.**
///
/// It is the fallback for a target whose shape nothing here knows, and
/// `wait_for_selectors` reads an empty set as "poll `document.readyState`
/// instead". A selector arriving here by accident would silently change what
/// every such target waits for.
#[test]
fn document_complete_carries_no_selectors() {
    assert!(
        ReadinessTarget::DocumentComplete.selectors().is_empty(),
        "DocumentComplete is the no-selector fallback and now carries {:?}",
        ReadinessTarget::DocumentComplete.selectors()
    );
}
