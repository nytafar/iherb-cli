//! Extraction provenance (#28): where every field came from, and whether the
//! record is healthy enough to trust.
//!
//! These are the tests values alone cannot write. `assert_eq!(p.upc, Some(..))`
//! passes whether the UPC came from JSON-LD or from a DOM fallback that
//! happened to find the same string, so it cannot notice a JSON-LD field
//! quietly starting to arrive from the DOM. That transition is degradation
//! *before* it becomes data loss, and asserting the source is what catches it.

use iherb_cli::model::{ProductDetail, Source, Strategy};
use iherb_cli::scraper::product::{
    enrich_from_html, parse_from_html, parse_from_js_globals, parse_from_json_ld,
};

use crate::fixture::{self, BASE_URL, OLLY_GUMMIES, TWO_A_DAY, ULTIMATE_OMEGA};

/// The production path for a product page: JSON-LD, then DOM enrichment.
fn as_production_would(f: crate::fixture::Fixture) -> ProductDetail {
    let mut product = parse_from_json_ld(&f.json_ld(), f.product_id(), BASE_URL)
        .unwrap_or_else(|| panic!("{}: no JSON-LD", f.slug()));
    enrich_from_html(f.html(), &mut product);
    product
}

/// **The source assertion.** Not one value is checked here; every assertion is
/// about where a value came from.
///
/// If iHerb drops `gtin12` from its JSON-LD tomorrow, `upc` keeps its value —
/// `extract_spec` finds the same digits in `#product-specs-list` — and every
/// value assertion in this suite goes on passing. This test fails, because the
/// source changed from `JsonLd` to `Dom`. That is the difference #28 is about.
#[test]
fn every_field_names_the_strategy_that_produced_it() {
    let product = as_production_would(TWO_A_DAY);

    // JSON-LD's own fields. `product_url` is not among them — no capture
    // publishes `url`, so every product URL is synthesised from the id, and
    // `product_url_is_never_read_from_a_page` below pins that.
    for field in [
        "name",
        "brand",
        "price",
        "original_price",
        "currency",
        "rating",
        "review_count",
        "in_stock",
        "description",
        "product_code",
        "upc",
    ] {
        assert_eq!(
            product.source_of(field),
            Source::JsonLd,
            "{} should still come from JSON-LD",
            field
        );
    }

    // Fields JSON-LD does not carry, filled by DOM enrichment.
    for field in [
        "ingredients",
        "supplement_facts",
        "suggested_use",
        "warnings",
    ] {
        assert_eq!(
            product.source_of(field),
            Source::Dom,
            "{} should come from the DOM",
            field
        );
    }

    // `shipping_weight` was in the list below until #2 landed. It was absent
    // because the label lookup was case-sensitive, not because the page lacked
    // it — exactly the distinction that used to be invisible: before
    // provenance, this was `None` and so was a field iHerb genuinely does not
    // publish. It now says `Dom`, which is where the page carries it.
    assert_eq!(product.source_of("shipping_weight"), Source::Dom);

    // `review_distribution` was in the list below until #32. It is `Dom` on
    // this page — the only capture whose histogram widget is hydrated.
    assert_eq!(product.source_of("review_distribution"), Source::Dom);

    // And the one nothing produced. The gummies page, which has no histogram
    // widget at all, still reports `review_distribution` absent — which is the
    // point of the distinction: `Absent` is the page saying nothing, not the
    // parser failing.
    assert_eq!(product.source_of("category_breadcrumb"), Source::Absent);
    assert_eq!(
        as_production_would(OLLY_GUMMIES).source_of("review_distribution"),
        Source::Absent
    );

    // And the one that has a value nobody read.
    assert_eq!(product.source_of("product_url"), Source::Defaulted);

    assert_eq!(product.extraction.strategy, Strategy::JsonLd);
    assert!(product.extraction.enriched);
}

/// A field nobody ever heard of is `Absent`, not a panic and not a `None` that
/// could be mistaken for a real answer.
#[test]
fn an_unknown_field_is_absent() {
    let product = as_production_would(TWO_A_DAY);
    assert_eq!(product.source_of("hairbrush_bristle_count"), Source::Absent);
}

/// The DOM strategy on its own records itself, and enriches like every other
/// path does.
#[test]
fn the_dom_strategy_records_itself_and_enriches() {
    let product = parse_from_html(ULTIMATE_OMEGA.html(), "12949", BASE_URL).unwrap();

    assert_eq!(product.extraction.strategy, Strategy::Dom);
    assert!(
        product.extraction.enriched,
        "every path enriches, so coverage does not depend on which strategy won"
    );
    assert_eq!(product.source_of("name"), Source::Dom);
    assert_eq!(product.source_of("upc"), Source::Dom);
}

/// Requirement 4 of #28, asserted rather than asserted-about. Both strategies
/// that can reach a captured page have to produce the same field coverage for
/// it — otherwise a page where JSON-LD happens to fail yields a systematically
/// thinner record with no warning.
///
/// Coverage, not values: `product_code` differs in *source* between the two
/// paths on some pages, but no longer in presence. The exclusion list this test
/// used to carry — `product_code` and `shipping_weight`, which #2 kept absent
/// on the DOM path — is gone, and its absence is the assertion: every tracked
/// field is now compared.
#[test]
fn every_strategy_produces_the_same_field_coverage() {
    for f in fixture::products() {
        let dom = parse_from_html(f.html(), f.product_id(), BASE_URL).unwrap();
        let json_ld = as_production_would(f);

        let dom_has: Vec<&str> = dom
            .field_presence()
            .into_iter()
            .filter(|(_, present)| *present)
            .map(|(name, _)| name)
            .collect();
        let ld_has: Vec<&str> = json_ld
            .field_presence()
            .into_iter()
            .filter(|(_, present)| *present)
            .map(|(name, _)| name)
            .collect();

        assert_eq!(
            dom_has,
            ld_has,
            "{}: the two strategies disagree about which fields the page has",
            f.slug()
        );
    }
}

/// The health block: what #9 renders under `--json`.
#[test]
fn a_scrape_reports_its_own_health() {
    let health = as_production_would(TWO_A_DAY).health();

    assert_eq!(health.strategy, Strategy::JsonLd);
    assert!(health.enriched);
    assert!(!health.degraded, "a complete page is not degraded");

    // Every tracked field appears, `Absent` included — that is the point.
    assert_eq!(health.sources.len(), 19);
    assert_eq!(health.sources["name"], Source::JsonLd);
    assert_eq!(health.sources["ingredients"], Source::Dom);
    assert_eq!(health.sources["shipping_weight"], Source::Dom);
    assert_eq!(health.sources["category_breadcrumb"], Source::Absent);

    assert!(health
        .fields_absent
        .contains(&"category_breadcrumb".to_string()));
    assert!(!health.fields_absent.contains(&"name".to_string()));
}

/// `degraded` means "our selectors rotted", not "this product has no supplement
/// facts because it is a hairbrush".
///
/// The gummies page has no ingredients, no suggested use and no warnings, and
/// is not degraded: those fields are legitimately absent. Neither is the DOM
/// path on the same page, now that #2 has landed — it used to be, because
/// `extract_spec` ate `product_code`, and that half of this test was the one
/// real degradation the captures could produce.
///
/// So the rot half is simulated instead of borrowed from a bug: renaming
/// `#product-specs-list` is what a selector rotting looks like from the
/// parser's side, and it costs the DOM path both `product_code` and `upc`.
#[test]
fn degraded_distinguishes_rotted_selectors_from_a_sparse_page() {
    let sparse = as_production_would(OLLY_GUMMIES).health();
    assert!(sparse.fields_absent.contains(&"ingredients".to_string()));
    assert!(sparse.fields_absent.contains(&"warnings".to_string()));
    assert!(
        !sparse.degraded,
        "a page that genuinely has no ingredients is sparse, not broken"
    );

    let via_dom = parse_from_html(OLLY_GUMMIES.html(), "119174", BASE_URL)
        .unwrap()
        .health();
    assert_eq!(via_dom.sources["product_code"], Source::Dom);
    assert!(
        !via_dom.degraded,
        "the DOM path reads every expected field off this page"
    );

    let renamed = OLLY_GUMMIES
        .html()
        .replace("product-specs-list", "product-specs-listicle");
    let rotted = parse_from_html(&renamed, "119174", BASE_URL)
        .unwrap()
        .health();
    assert_eq!(rotted.sources["product_code"], Source::Absent);
    assert_eq!(rotted.sources["upc"], Source::Absent);
    assert!(
        rotted.degraded,
        "product_code and upc are in EXPECTED_FIELDS, so losing them is degradation"
    );
}

/// Every field named in `EXPECTED_FIELDS` is a field the tracked set knows
/// about. A typo there would silently make `degraded` permanently true.
#[test]
fn the_expected_field_names_are_real_field_names() {
    let tracked: Vec<&str> = as_production_would(TWO_A_DAY)
        .field_presence()
        .into_iter()
        .map(|(name, _)| name)
        .collect();

    for expected in ProductDetail::EXPECTED_FIELDS {
        assert!(
            tracked.contains(expected),
            "EXPECTED_FIELDS names {:?}, which is not a tracked field",
            expected
        );
    }
}

/// First writer wins. DOM enrichment fills gaps; it does not relabel what a
/// more trusted strategy already produced.
#[test]
fn enrichment_does_not_relabel_what_json_ld_produced() {
    let mut product = parse_from_json_ld(&TWO_A_DAY.json_ld(), "104996", BASE_URL).unwrap();
    assert_eq!(product.source_of("rating"), Source::JsonLd);

    enrich_from_html(TWO_A_DAY.html(), &mut product);

    // `enrich_rating_and_reviews` would have read the same rating off the stars
    // if JSON-LD had not supplied one. It did, so the source is unchanged.
    assert_eq!(product.source_of("rating"), Source::JsonLd);
    assert_eq!(product.rating, Some(4.7));
}

/// A record nothing extracted says so, rather than pretending to a strategy.
#[test]
fn a_hand_built_record_is_unrecorded() {
    let health = ProductDetail {
        name: "Handmade".to_string(),
        brand: String::new(),
        price: 1.0,
        original_price: None,
        currency: Some("USD".to_string()),
        rating: None,
        review_count: None,
        product_url: String::new(),
        product_id: "1".to_string(),
        in_stock: None,
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
        extraction: Default::default(),
    }
    .health();

    assert_eq!(health.strategy, Strategy::Unrecorded);
    assert!(!health.enriched);
    // Nothing claimed anything, so everything is absent — including `name`,
    // which has a value. Provenance reports what was recorded, never what it
    // can infer after the fact.
    assert_eq!(health.fields_absent.len(), 19);
    assert!(health.degraded);
}

/// A cache file written before provenance existed still loads, and comes back
/// honest about knowing nothing.
#[test]
fn a_pre_provenance_cache_entry_still_deserializes() {
    let json = serde_json::json!({
        "name": "California Gold Nutrition, Gold C",
        "brand": "California Gold Nutrition",
        "price": 9.6,
        "original_price": null,
        "currency": "USD",
        "rating": 4.8,
        "review_count": 381864,
        "product_url": "https://www.iherb.com/pr/item/61864",
        "product_id": "61864",
        "in_stock": true,
        "description": null,
        "product_code": null,
        "upc": null,
        "ingredients": null,
        "supplement_facts": null,
        "suggested_use": null,
        "warnings": null,
        "shipping_weight": null,
        "category_breadcrumb": null,
        "review_distribution": null,
    });

    let product: ProductDetail = serde_json::from_value(json).expect("old cache entries must load");
    assert_eq!(product.in_stock, Some(true));
    assert_eq!(product.extraction.strategy, Strategy::Unrecorded);
    assert_eq!(product.source_of("name"), Source::Absent);
}

/// The JSON shape #9 has to emit, pinned here so #9 does not have to invent it.
#[test]
fn health_serializes_to_the_block_issue_9_renders() {
    let health = as_production_would(TWO_A_DAY).health();
    let json = serde_json::to_value(&health).expect("ExtractionHealth must serialize");

    assert_eq!(json["strategy"], "json_ld");
    assert_eq!(json["enriched"], true);
    assert_eq!(json["degraded"], false);
    assert_eq!(json["sources"]["name"], "json_ld");
    assert_eq!(json["sources"]["ingredients"], "dom");
    assert_eq!(json["sources"]["shipping_weight"], "dom");
    assert_eq!(json["sources"]["category_breadcrumb"], "absent");
    assert_eq!(json["sources"]["product_url"], "defaulted");
    assert!(json["fields_absent"].is_array());
    assert!(json["fields_defaulted"].is_array());
    // Added this round (#32). #9 renders `health()` verbatim, so a new key in
    // the block is a contract change and is pinned here rather than discovered.
    assert!(json["fields_malformed"].is_array());
    assert_eq!(json["fields_malformed"].as_array().unwrap().len(), 0);

    let via_globals = serde_json::json!(Strategy::JsGlobals);
    assert_eq!(via_globals, "js_globals");
    assert_eq!(serde_json::json!(Source::Defaulted), "defaulted");
    assert_eq!(serde_json::json!(Source::Malformed), "malformed");
    assert_eq!(serde_json::json!(Strategy::Unrecorded), "unrecorded");
}

// ---------------------------------------------------------------------------
// Values nobody read: `Source::Defaulted`
// ---------------------------------------------------------------------------

/// Provenance must not vouch for a value nobody read off the page.
///
/// `currency` is the live case. Every path falls back to a constant `"USD"` or
/// to whatever label the caller passed to `--currency`, and before this the
/// fallback was recorded as though the strategy had read it. That made the
/// `currency` slot in `EXPECTED_FIELDS` a rot-detector that could never fire:
/// the field is always non-empty, so its source was always attested, so it
/// could never contribute to `degraded` however thoroughly the currency
/// selectors rotted.
///
/// #49 stopped provenance lying about the value; #5 removed the value. Offers
/// with no `priceCurrency` used to become a hardcoded `"USD"` recorded as
/// [`Source::Defaulted`]; they now become no currency at all, recorded as
/// [`Source::Absent`]. Both are unattested, which is what this test is for.
#[test]
fn an_undetected_currency_is_not_attributed_to_a_strategy() {
    // JSON-LD with a price but no `priceCurrency` anywhere.
    let no_currency = serde_json::json!({
        "@type": "Product",
        "name": "Thing",
        "offers": { "price": "9.60" },
    });
    let product = parse_from_json_ld(&no_currency, "1", BASE_URL).unwrap();

    // There is no `"USD"` to inherit any more: `9.60` of nothing named.
    assert_eq!(product.currency, None);
    assert_eq!(product.source_of("currency"), Source::Absent);
    assert!(!product.source_of("currency").is_attested());
}

/// The rot-detector fires on a field that was not read, whether it is absent or
/// merely defaulted. Either way its slot in `EXPECTED_FIELDS` has to be able to
/// make `degraded` true, or the slot is dead weight.
///
/// The parser now reaches the absent half: offers with no `priceCurrency`
/// produce no currency. Before #5 the same offers produced a hardcoded `"USD"`
/// and reached the defaulted half, which is asserted below on a record built by
/// hand, since nothing produces a defaulted expected field any more.
#[test]
fn an_unread_expected_field_makes_the_record_degraded() {
    assert!(
        ProductDetail::EXPECTED_FIELDS.contains(&"currency"),
        "this test is about currency's slot in the expected set"
    );

    let no_currency = serde_json::json!({
        "@type": "Product",
        "name": "Thing",
        "brand": { "name": "Acme" },
        "sku": "ACM-1",
        "gtin12": "000000000001",
        "offers": { "price": "9.60", "availability": "https://schema.org/InStock" },
    });
    let mut product = parse_from_json_ld(&no_currency, "1", BASE_URL).unwrap();
    let health = product.health();

    // Everything else in the expected set was read.
    assert!(health.fields_absent.is_empty() || !health.fields_absent.contains(&"name".to_string()));
    assert!(health.fields_absent.contains(&"currency".to_string()));
    assert!(
        health.degraded,
        "a currency nobody read is exactly the 'our selectors rotted' signal"
    );

    // The same record with a value nobody read, rather than no value at all.
    product.currency = Some("USD".to_string());
    product.extraction.reclaim("currency", Source::Defaulted);
    let defaulted = product.health();
    assert!(defaulted.fields_defaulted.contains(&"currency".to_string()));
    assert!(
        defaulted.degraded,
        "a value nobody read must degrade the record exactly as a missing one does"
    );

    // And the same page with a currency is not degraded, so `degraded` is
    // tracking the currency and not something incidental.
    let mut with_currency = no_currency.clone();
    with_currency["offers"]["priceCurrency"] = serde_json::json!("CHF");
    let ok = parse_from_json_ld(&with_currency, "1", BASE_URL)
        .unwrap()
        .health();
    assert_eq!(ok.sources["currency"], Source::JsonLd);
    assert!(!ok.degraded);
}

/// `Defaulted` and `Absent` are different answers, and the health block keeps
/// them apart. A caller can ignore an absent field; a defaulted one will
/// silently look like data.
#[test]
fn defaulted_is_not_absent() {
    assert!(!Source::Defaulted.is_attested());
    assert!(!Source::Absent.is_attested());
    // Malformed is not attested either: nothing was read. It is nonetheless a
    // third answer, and `a_malformed_histogram_degrades_the_record` in
    // `product_dom` is where the three are held apart.
    assert!(!Source::Malformed.is_attested());
    assert!(Source::JsonLd.is_attested());
    assert!(Source::JsGlobals.is_attested());
    assert!(Source::Dom.is_attested());

    let health = as_production_would(TWO_A_DAY).health();
    assert!(health.fields_defaulted.contains(&"product_url".to_string()));
    assert!(!health.fields_absent.contains(&"product_url".to_string()));
    assert!(health
        .fields_absent
        .contains(&"category_breadcrumb".to_string()));
    assert!(!health
        .fields_defaulted
        .contains(&"category_breadcrumb".to_string()));
}

/// The captures all publish a currency, so none of them is degraded on that
/// account — which is what keeps the change from being a blanket "everything is
/// degraded now".
#[test]
fn every_captured_page_publishes_a_currency() {
    for f in fixture::products() {
        let via_json_ld = as_production_would(f);
        assert_eq!(
            via_json_ld.source_of("currency"),
            Source::JsonLd,
            "{}: JSON-LD offers carry priceCurrency",
            f.slug()
        );

        let via_dom = parse_from_html(f.html(), f.product_id(), BASE_URL).unwrap();
        assert_eq!(
            via_dom.source_of("currency"),
            Source::Dom,
            "{}: the page carries a currency marker",
            f.slug()
        );
    }
}

/// The JS-globals blob carries no currency at all, so that path produces no
/// currency at all.
///
/// It used to produce the caller's `--currency` label, which #49 recorded as
/// [`Source::Defaulted`] rather than attributing to the strategy. #5 went the
/// rest of the way: a label from the command line is not a reading of the page,
/// so there is nothing to record. The record is still degraded — the blob
/// answering nothing about currency is exactly what `EXPECTED_FIELDS` is for.
#[test]
fn the_js_globals_path_produces_no_currency() {
    let globals = fixture::json("js-globals-12949");
    let product = parse_from_js_globals(&globals, "12949", BASE_URL).unwrap();

    assert_eq!(product.currency, None);
    assert_eq!(product.source_of("currency"), Source::Absent);
    assert!(
        product.health().degraded,
        "a record with no currency is not extraction succeeding"
    );

    // The fields the blob really does carry are still attributed to it.
    assert_eq!(product.source_of("name"), Source::JsGlobals);
    assert_eq!(product.source_of("upc"), Source::JsGlobals);
}

/// No capture publishes a canonical `url`, on any path, so the product URL is
/// always built from the id. Same class as currency: a real value that no
/// strategy read.
#[test]
fn product_url_is_never_read_from_a_page() {
    for f in fixture::products() {
        let via_json_ld = as_production_would(f);
        assert_eq!(
            via_json_ld.source_of("product_url"),
            Source::Defaulted,
            "{}",
            f.slug()
        );
        assert_eq!(
            via_json_ld.product_url,
            format!("{}/pr/p/{}", BASE_URL, f.product_id())
        );

        let via_dom = parse_from_html(f.html(), f.product_id(), BASE_URL).unwrap();
        assert_eq!(
            via_dom.source_of("product_url"),
            Source::Defaulted,
            "{}",
            f.slug()
        );
    }

    // A block that does publish one is attributed to it.
    let with_url = serde_json::json!({
        "@type": "Product",
        "name": "Thing",
        "url": "https://www.iherb.com/pr/thing/1",
    });
    let product = parse_from_json_ld(&with_url, "1", BASE_URL).unwrap();
    assert_eq!(product.source_of("product_url"), Source::JsonLd);
    assert_eq!(product.product_url, "https://www.iherb.com/pr/thing/1");
}
