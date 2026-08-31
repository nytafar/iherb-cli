//! The parsers fed page HTML or a parsed document: `parse_from_html`,
//! `enrich_from_html`, `extract_spec`, `parse_supplement_facts_html`,
//! `parse_review_distribution_html`.

use iherb_cli::error::IherbError;
use iherb_cli::model::{ReviewDistribution, Source};
use iherb_cli::scraper::helpers::is_not_found_page;
use iherb_cli::scraper::product::{
    enrich_from_html, extract_spec, parse_from_html, parse_from_json_ld, parse_product_specs,
    parse_review_distribution_html, parse_supplement_facts_html, HistogramFault, HistogramRead,
};

use crate::fixture::{
    self, BASE_URL, BUTYRATE_TWO_CAP_SERVING, B_COMPLEX, DENTALCIDIN_TUBE, FIBERAID_POWDER,
    GOLD_C_POWDER, LITHIUM_MICRO_TABLETS, OLLY_GUMMIES, R_LIPOIC_TINY_ID, SUPREME_C_TABLETS,
    TART_CHERRY_LIQUID, TWO_A_DAY, ULTIMATE_OMEGA, ULTIMATE_OMEGA_NOK,
};

// ---------------------------------------------------------------------------
// parse_from_html — the last-resort DOM fallback
// ---------------------------------------------------------------------------

#[test]
fn dom_fallback_reads_every_product_page() {
    for f in fixture::products() {
        let product = parse_from_html(f.html(), f.product_id(), BASE_URL)
            .unwrap_or_else(|e| panic!("{}: {}", f.slug(), e));

        assert!(!product.name.is_empty(), "{}", f.slug());
        assert!(!product.brand.is_empty(), "{}", f.slug());
        assert!(product.price > 0.0, "{}", f.slug());
        assert!(product.rating.is_some(), "{}", f.slug());
        assert!(product.review_count.is_some(), "{}", f.slug());
        assert!(product.upc.is_some(), "{}", f.slug());
        assert!(product.suggested_use.is_some() || f.slug() == OLLY_GUMMIES.slug());
    }
}

/// The DOM fallback reaches the same headline numbers as JSON-LD, which is what
/// makes it a usable fallback rather than a different answer.
#[test]
fn dom_fallback_agrees_with_json_ld_on_price_and_rating() {
    for f in fixture::products() {
        let dom = parse_from_html(f.html(), f.product_id(), BASE_URL).unwrap();
        let ld = parse_from_json_ld(&f.json_ld(), f.product_id(), BASE_URL).unwrap();

        assert_eq!(dom.name, ld.name, "{}", f.slug());
        assert_eq!(dom.brand, ld.brand, "{}", f.slug());
        assert_eq!(dom.price, ld.price, "{}", f.slug());
        assert_eq!(dom.original_price, ld.original_price, "{}", f.slug());
        assert_eq!(dom.rating, ld.rating, "{}", f.slug());
        assert_eq!(dom.review_count, ld.review_count, "{}", f.slug());
        assert_eq!(dom.upc, ld.upc, "{}", f.slug());
    }
}

/// #2, flipped. This was `dom_fallback_loses_product_code_and_shipping_weight`.
///
/// The DOM fallback used to lose `product_code` and `shipping_weight` on every
/// page, because `extract_spec` was asked for `"Product Code"` /
/// `"Shipping Weight"` and the page says `Product code:` / `Shipping weight:`.
/// `UPC` survived only because it is an initialism and the case happened to
/// match. The lookup is case-insensitive now, so all three resolve.
///
/// The values are asserted, not just their presence: a case fix alone would
/// hand `shipping_weight` back with the info tooltip glued on, which is a
/// different bug wearing the same `Some`.
#[test]
fn dom_fallback_reads_product_code_and_shipping_weight() {
    for f in fixture::products() {
        let product = parse_from_html(f.html(), f.product_id(), f.base_url()).unwrap();
        assert!(product.product_code.is_some(), "{}", f.slug());
        assert!(product.upc.is_some(), "{}", f.slug());

        let weight = product
            .shipping_weight
            .unwrap_or_else(|| panic!("{}: no shipping weight", f.slug()));

        // `lb` is the US storefront's unit, not a fact about the field: the
        // Norwegian capture says `kg`. The tooltip check is the point of the
        // assertion and applies to every page; the unit is asserted only where
        // it is the page's own.
        if f.base_url() == fixture::US_STOREFRONT {
            assert!(
                weight.ends_with(" lb"),
                "{}: shipping weight is a bare weight, not the tooltip too: {:?}",
                f.slug(),
                weight
            );
        }
    }

    let nordic = parse_from_html(ULTIMATE_OMEGA.html(), "12949", BASE_URL).unwrap();
    assert_eq!(nordic.product_code.as_deref(), Some("NOR-03790"));
    assert_eq!(nordic.shipping_weight.as_deref(), Some("0.72 lb"));
}

/// The same product, two storefronts, two currencies, two prices (#5).
///
/// This is what `--currency` buys, asserted against the two real captures
/// rather than against the flag. The pair is only obtainable because the
/// preference cookies change the document iHerb serves: no URL selects a
/// currency, and from the IP that captured these, both requests would otherwise
/// have come back Norwegian.
///
/// It is also the test that would have caught the bug the whole issue is about.
/// Before #5, the currency on a record was `--currency`'s label wherever
/// detection fell through, so a US price and a Norwegian price could carry the
/// same currency string. Here they cannot: each page states its own, and the
/// numbers differ by more than a factor of ten.
#[test]
fn one_product_prices_differently_on_two_storefronts() {
    let usd = parse_from_html(ULTIMATE_OMEGA.html(), "12949", ULTIMATE_OMEGA.base_url()).unwrap();
    let nok = parse_from_html(
        ULTIMATE_OMEGA_NOK.html(),
        "12949",
        ULTIMATE_OMEGA_NOK.base_url(),
    )
    .unwrap();

    // The same product.
    assert_eq!(usd.product_id, nok.product_id);
    assert_eq!(usd.product_code, nok.product_code);
    assert_eq!(usd.upc, nok.upc);

    // Priced by two storefronts, each naming its own currency — read off the
    // page on both, never substituted.
    assert_eq!(usd.currency.as_deref(), Some("USD"));
    assert_eq!(nok.currency.as_deref(), Some("NOK"));
    assert_eq!(usd.source_of("currency"), Source::Dom);
    assert_eq!(nok.source_of("currency"), Source::Dom);

    // And the numbers are the storefronts' own, not one number relabelled.
    assert!(
        nok.price > usd.price * 5.0,
        "{} vs {}",
        nok.price,
        usd.price
    );
    assert_eq!(nok.price, 880.63);

    // A Norwegian price parses as a price: no thousands separator confusion, no
    // currency prefix left in the number.
    assert!(nok.price.fract() > 0.0);
}

/// CHARACTERIZATION, NOT DESIRED: `shipping_weight` on a page captured from the
/// **current** site carries the info tooltip glued to the value.
///
/// Not a regression in our code and not a storefront difference — a change in
/// iHerb's markup. The seven upstream captures are siteVersion 1.0.19891 to
/// 1.0.20071 and give a bare `0.72 lb`; the NOK capture is 1.0.22698 and gives
/// the weight followed by the whole "The Shipping Weight includes the product,
/// protective packaging material…" tooltip. `extract_spec` takes the text of the
/// value cell, and that cell now contains the tooltip too.
///
/// This is live: `iherb-cli product 12949 --country no` prints the paragraph
/// today. #5 found it by capturing a page newer than the fixtures and is not
/// fixing it — this pins it so that whoever does can see it go green, and so
/// that the desired value is written down.
///
/// DESIRED: `Some("0.33 kg")`.
#[test]
fn shipping_weight_on_a_current_page_carries_the_tooltip_too() {
    let nok = parse_from_html(
        ULTIMATE_OMEGA_NOK.html(),
        "12949",
        ULTIMATE_OMEGA_NOK.base_url(),
    )
    .unwrap();
    let weight = nok.shipping_weight.expect("the page publishes a weight");

    assert!(weight.starts_with("0.33 kg"), "{:?}", weight);
    assert!(
        weight.contains("The Shipping Weight includes the product"),
        "if this stopped being true the tooltip is gone and the DESIRED value \
         above is what to assert instead: {:?}",
        weight
    );
}

/// #31, flipped. This was `dom_fallback_reports_the_gummies_as_in_stock`.
///
/// The gummies page is out of stock and says so four separate ways. The DOM
/// fallback used to report it in stock, because `#stock-status` is absent on
/// that page and the default was `!html.contains("Out of Stock")` — which the
/// page satisfies only because it writes "Out of stock" with a lower-case s.
///
/// JSON-LD is not consulted anywhere in this test: `parse_from_html` is the DOM
/// path on its own, which is the point. The DOM has to reach the same answer
/// JSON-LD does, or the fallback is not a fallback.
#[test]
fn dom_fallback_reads_the_gummies_as_out_of_stock() {
    let product = parse_from_html(OLLY_GUMMIES.html(), "119174", BASE_URL).unwrap();
    assert_eq!(product.in_stock, Some(false));

    // The exact-case substring the old default relied on is still not there.
    // That is what made the bug invisible, and it is still true, so this test
    // is not passing because the page changed.
    assert!(!OLLY_GUMMIES.html().contains("Out of Stock"));
}

/// The four signals the gummies page carries, read straight from the capture.
/// If a re-capture drops one, this test says which — and
/// `dom_fallback_reads_the_gummies_as_out_of_stock` above says whether the
/// remaining ones are still enough.
#[test]
fn the_gummies_page_says_out_of_stock_four_ways() {
    let html = OLLY_GUMMIES.html();
    assert!(html.contains(r#""availability":"https://schema.org/OutOfStock""#));
    assert!(html.contains(r#"data-stock-status="Out of stock""#));
    assert!(html.contains(r#"data-is-out-of-stock="True""#));
    assert!(html.contains(r#"stckInd: "OutOfStock""#));

    // And the product-level numeric code, which is `0` on all four in-stock
    // captures.
    assert!(html.contains(r#"data-stock-status="3""#));
}

/// The DOM path agrees with JSON-LD about availability on every capture. This
/// is the assertion that would have caught #31 the day it was written: the two
/// paths disagreed on the gummies and nothing said so.
#[test]
fn dom_and_json_ld_agree_about_availability() {
    for f in fixture::products() {
        let dom = parse_from_html(f.html(), f.product_id(), BASE_URL).unwrap();
        let ld = parse_from_json_ld(&f.json_ld(), f.product_id(), BASE_URL).unwrap();
        assert_eq!(dom.in_stock, ld.in_stock, "{}", f.slug());
        assert!(dom.in_stock.is_some(), "{}", f.slug());
    }
}

/// The variant signal is scoped to this product's own id on purpose. The
/// B-Complex page is in stock and carries `data-is-out-of-stock="True"` — on
/// the 30-count variant, product 108265, which is a different product. A
/// page-wide substring search would report the page out of stock.
///
/// This assertion alone does NOT protect the scoping: B-Complex has a
/// `#stock-status` heading saying "In stock", which the reader consults first,
/// so the variant rung never runs on this page and removing the scoping changes
/// nothing here. `variant_scoping_survives_a_page_with_no_stock_status_heading`
/// below is the one that bites. Both are kept: this one pins the shape of the
/// page, that one pins the behaviour.
#[test]
fn an_out_of_stock_sibling_variant_does_not_condemn_the_page() {
    let html = B_COMPLEX.html();
    assert!(html.contains(r#"data-is-out-of-stock="True""#));

    let product = parse_from_html(html, "108255", BASE_URL).unwrap();
    assert_eq!(product.in_stock, Some(true));
}

/// The test that fails the moment the variant scoping is removed.
///
/// The subtlety it protects: `[data-pid="<id>"][data-is-out-of-stock]` is
/// matched against *this* product's id, not read off whichever element in the
/// document happens to carry the attribute first. On the B-Complex page the
/// first one in document order is product **108265**, the 30-count sibling
/// variant, and it says `"True"`. Simplify the selector to
/// `[data-is-out-of-stock]` and an in-stock page reports out of stock.
///
/// Reaching that rung takes some doing, which is exactly why the assertion
/// above cannot protect it. The reader consults `#stock-status` first, and
/// B-Complex has one. So this test neutralises that element and nothing else —
/// producing a page shaped like the gummies capture, which genuinely has no
/// `#stock-status` at all, but with an in-stock product and an out-of-stock
/// sibling. That combination is the one that tells correct from naive, and no
/// captured page has it on its own.
#[test]
fn variant_scoping_survives_a_page_with_no_stock_status_heading() {
    // Rename the id rather than cutting the element out, so the surgery cannot
    // disturb the document structure the rest of the parse depends on.
    let html = B_COMPLEX.html().replace(
        r#"id="stock-status""#,
        r#"id="stock-status-disabled-by-this-test""#,
    );

    // The surgery has to have done something, or this test quietly stops
    // testing anything — which is the failure mode it exists to fix.
    assert!(
        B_COMPLEX.html().contains(r#"id="stock-status""#),
        "the capture must have the heading for removing it to mean anything"
    );
    assert!(!html.contains(r#"id="stock-status""#));

    // The two variants, and the order they appear in. The sibling comes first,
    // so a selector that takes the first match takes the wrong one.
    assert!(
        html.find(r#"data-pid="108265""#) < html.find(r#"data-pid="108255""#),
        "the out-of-stock sibling must come first, or the naive selector \
         would accidentally be right"
    );

    let product = parse_from_html(&html, "108255", BASE_URL).unwrap();
    assert_eq!(
        product.in_stock,
        Some(true),
        "product 108255 is in stock; 108265 is the sibling variant that is not. \
         Reading the variant signal without scoping it to the requested product \
         id reports this page out of stock."
    );
}

/// A page with none of the signals is unknown, not in stock. This is the case
/// #28 is about: absent and broken must not both look like an answer.
#[test]
fn a_page_with_no_stock_signal_answers_unknown() {
    let bare = r#"<html><body><h1 id="name">Some Product</h1></body></html>"#;
    let product = parse_from_html(bare, "1", BASE_URL).unwrap();
    assert_eq!(product.in_stock, None);
}

/// FLIPPED BY #5. This was the characterization from the parser side: the
/// function took a `currency` argument, the captured US pages carried `USD` and
/// beat it, and a page carrying no marker got the argument stamped on instead —
/// so `--currency CHF` against the US storefront produced either USD prices
/// labelled USD or USD prices labelled CHF, depending only on whether detection
/// happened to work.
///
/// There is no argument to ignore any more. The DOM path reports the currency
/// the page published, and reports nothing when the page published nothing.
#[test]
fn the_dom_path_reports_the_currency_the_page_published() {
    for f in fixture::products() {
        let product = parse_from_html(f.html(), f.product_id(), f.base_url()).unwrap();
        assert_eq!(
            product.currency.as_deref(),
            Some(f.currency()),
            "{}",
            f.slug()
        );
        assert_eq!(product.source_of("currency"), Source::Dom, "{}", f.slug());
    }

    // A product page with no currency marker anywhere. Before #5 this record
    // carried a currency; now it carries the absence of one, and says so.
    let unmarked = parse_from_html(
        r#"<html><body><h1 id="name">Some Product</h1>
           <span class="price"><bdi>9.60</bdi></span></body></html>"#,
        "1",
        BASE_URL,
    )
    .unwrap();
    assert_eq!(unmarked.currency, None);
    assert_eq!(unmarked.source_of("currency"), Source::Absent);
    assert!(
        unmarked.health().degraded,
        "a price with no currency is not extraction succeeding"
    );
}

/// Two rejections that used to be the same one (#28).
///
/// A page that says the product is gone is `ProductNotFound` — stop asking. A
/// page that loaded fine and yielded no name is `ParseFailed` — the selectors
/// are broken, the id is probably fine, and a human should look. Collapsing
/// them into "product not found" is what tells a caller to give up on a valid
/// id, which is the misclassification the whole issue is about.
#[test]
fn a_page_that_will_not_parse_is_not_a_missing_product() {
    let err = parse_from_html("<html><body><p>nothing</p></body></html>", "1", BASE_URL)
        .expect_err("a page with no h1 is not a product page");
    assert!(
        matches!(err, IherbError::ParseFailed(ref id) if id == "1"),
        "expected ParseFailed, got {:?}",
        err
    );
    assert!(err.to_string().contains('1'));

    let err = parse_from_html("<html><title>404</title>", "42", BASE_URL)
        .expect_err("a 404 page is not a product page");
    assert!(
        matches!(err, IherbError::ProductNotFound(ref id) if id == "42"),
        "expected ProductNotFound, got {:?}",
        err
    );
    assert!(err.to_string().contains("42"));
}

#[test]
fn captured_pages_are_not_mistaken_for_404s() {
    for f in fixture::all() {
        assert!(!is_not_found_page(f.html()), "{}", f.slug());
    }
    assert!(is_not_found_page("<html><title>404</title></html>"));
    assert!(is_not_found_page("<h1>Page Not Found</h1>"));
}

// ---------------------------------------------------------------------------
// enrich_from_html — what JSON-LD cannot supply
// ---------------------------------------------------------------------------

/// The production path: JSON-LD for the core fields, then the DOM for the
/// sections and the supplement table.
#[test]
fn enrichment_adds_the_dom_only_sections() {
    let mut product = parse_from_json_ld(&TWO_A_DAY.json_ld(), "104996", BASE_URL).unwrap();
    assert!(product.supplement_facts.is_none());
    assert!(product.ingredients.is_none());

    enrich_from_html(TWO_A_DAY.html(), &mut product);

    assert!(product.supplement_facts.is_some());
    assert!(product.ingredients.is_some());
    assert!(product.suggested_use.is_some());
    assert!(product.warnings.is_some());
    // Enrichment must not disturb what JSON-LD already established.
    assert_eq!(product.price, 12.38);
    assert_eq!(product.original_price, Some(17.69));
    assert_eq!(product.rating, Some(4.7));
}

/// The gummies page has no `.prodOverviewIngred` and no `#product-overview h3`
/// sections, so enrichment adds only the supplement table. A parser that starts
/// returning `Some` here has found something the page does not have.
#[test]
fn enrichment_adds_nothing_it_cannot_find() {
    let mut product = parse_from_json_ld(&OLLY_GUMMIES.json_ld(), "119174", BASE_URL).unwrap();
    enrich_from_html(OLLY_GUMMIES.html(), &mut product);

    assert!(product.supplement_facts.is_some());
    assert!(product.ingredients.is_none());
    assert!(product.suggested_use.is_none());
    assert!(product.warnings.is_none());
    assert_eq!(
        product.in_stock,
        Some(false),
        "JSON-LD's OutOfStock must survive enrichment"
    );
}

// ---------------------------------------------------------------------------
// extract_spec — #product-specs-list
// ---------------------------------------------------------------------------

/// #2, flipped. This was `extract_spec_matches_labels_case_sensitively`.
///
/// The match was `text.starts_with(label)`, so the label's case had to be
/// exactly right and the two lookups production asks for in title case could
/// never resolve. Both cases answer now, and answer the same thing.
///
/// Lowercasing the comparison was necessary but not sufficient: `Shipping
/// weight` matched once the case was right, and then came back with the whole
/// 500-word info tooltip glued on, because the tooltip lives inside the same
/// `<li>` and the old parser took everything after the first colon. The last
/// assertion here is the one that catches a half-fix.
#[test]
fn extract_spec_matches_labels_whatever_their_case() {
    let doc = ULTIMATE_OMEGA.doc();

    // What production asks for.
    assert_eq!(
        extract_spec(&doc, "Product Code").as_deref(),
        Some("NOR-03790")
    );
    assert_eq!(
        extract_spec(&doc, "Shipping Weight").as_deref(),
        Some("0.72 lb")
    );
    assert_eq!(extract_spec(&doc, "UPC").as_deref(), Some("768990037900"));

    // What the page actually says, and the shouted form neither uses.
    assert_eq!(
        extract_spec(&doc, "Product code").as_deref(),
        Some("NOR-03790")
    );
    assert_eq!(
        extract_spec(&doc, "SHIPPING WEIGHT").as_deref(),
        Some("0.72 lb")
    );

    // The tooltip lives in the `Shipping weight` row and is not the value.
    let weight = extract_spec(&doc, "Shipping weight").expect("the page has one");
    assert!(
        !weight.contains("The Shipping Weight includes the product"),
        "the tooltip is glued to the value again: {:?}",
        weight
    );
}

/// The whole list in one call, which is what `extract_spec` looks up in.
///
/// Six rows on every capture, in page order, values bounded. `Dimensions` is
/// the row that proves the value is read rather than reassembled: its two
/// `<span>`s are joined by a comma that belongs to the page, and on three of
/// the five captures that comma sits on its own source line.
#[test]
fn the_whole_spec_list_parses_in_page_order() {
    for f in fixture::products() {
        let labels: Vec<String> = parse_product_specs(&f.doc())
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        assert_eq!(
            labels,
            [
                "First available",
                "Shipping weight",
                "Product code",
                "UPC",
                "Package quantity",
                "Dimensions",
            ],
            "{}",
            f.slug()
        );
    }

    let nordic = parse_product_specs(&ULTIMATE_OMEGA.doc());
    assert_eq!(
        nordic,
        [
            ("First available".to_string(), "03/2019".to_string()),
            ("Shipping weight".to_string(), "0.72 lb".to_string()),
            ("Product code".to_string(), "NOR-03790".to_string()),
            ("UPC".to_string(), "768990037900".to_string()),
            ("Package quantity".to_string(), "180 count".to_string()),
            (
                "Dimensions".to_string(),
                "5.85 x 3.2 x 3.15 in, 0.72 lb".to_string()
            ),
        ]
    );
}

/// Every capture has a `#product-specs-list`, the gummies page included — the
/// three labels below resolve on all five. `Package quantity` and
/// `First available` are reachable too; nothing on `ProductDetail` has a home
/// for them yet, which is the separate specs-extraction issue's business.
#[test]
fn every_product_page_has_a_spec_list() {
    for f in fixture::products() {
        let doc = f.doc();
        assert!(extract_spec(&doc, "Product code").is_some(), "{}", f.slug());
        assert!(extract_spec(&doc, "UPC").is_some(), "{}", f.slug());
        assert!(
            extract_spec(&doc, "Package quantity").is_some(),
            "{}",
            f.slug()
        );
        assert!(
            extract_spec(&doc, "First available").is_some(),
            "{}",
            f.slug()
        );
    }

    let gummies = OLLY_GUMMIES.doc();
    assert_eq!(
        extract_spec(&gummies, "Product code").as_deref(),
        Some("OLE-00570")
    );
    assert_eq!(
        extract_spec(&gummies, "Package quantity").as_deref(),
        Some("42 count")
    );
    assert_eq!(
        extract_spec(&gummies, "First available").as_deref(),
        Some("04/2023")
    );
}

#[test]
fn extract_spec_returns_none_for_a_label_that_is_not_there() {
    let doc = ULTIMATE_OMEGA.doc();
    assert_eq!(extract_spec(&doc, "Country of origin"), None);
    assert_eq!(extract_spec(&fixture::empty_doc(), "UPC"), None);

    // Whole label, not a prefix. The match used to be `starts_with`, which let
    // a half-typed label answer with a neighbouring row's value.
    assert_eq!(extract_spec(&doc, "Product"), None);
    assert_eq!(extract_spec(&doc, "Shipping"), None);
}

// ---------------------------------------------------------------------------
// parse_supplement_facts_html
// ---------------------------------------------------------------------------

/// Every product page that carries a Supplement Facts panel parses it, and what
/// parses always has nutrients and a serving size.
///
/// This sweep used to run over `products()` unconditionally and to require
/// `servings_per_container` as well. Both claims were true of the eight pages
/// it could see, and both were false about iHerb: all eight were swallowable
/// US dietary supplements, so "every product page has a facts panel" and "every
/// panel states servings per container" were assertions with no available
/// counterexample. #8's Norwegian corpus supplied five on its first run — see
/// the two tests below, which is where each dropped claim went.
#[test]
fn supplement_facts_parse_wherever_a_page_carries_them() {
    for f in fixture::products() {
        // The one page with no panel at all has its own test, immediately below.
        if f.slug() == DENTALCIDIN_TUBE.slug() {
            continue;
        }
        let facts = parse_supplement_facts_html(&f.doc())
            .unwrap_or_else(|| panic!("{}: no supplement facts", f.slug()));
        assert!(!facts.nutrients.is_empty(), "{}", f.slug());
        assert!(facts.serving_size.is_some(), "{}", f.slug());
    }
}

/// Exactly one product page has no Supplement Facts panel, and `None` is the
/// correct reading of it rather than a parse failure: Dentalcidin is a
/// toothpaste. It is a product iHerb sells and this tool must describe, and it
/// has no serving, no daily values and nothing to state them about.
///
/// Asserted as a set rather than as a single `is_none()` so it also fails the
/// other way — if any *other* page stops producing a panel, this is what says
/// which one.
#[test]
fn one_product_page_carries_no_supplement_facts_and_it_is_the_toothpaste() {
    let without: Vec<&str> = fixture::products()
        .filter(|f| parse_supplement_facts_html(&f.doc()).is_none())
        .map(|f| f.slug())
        .collect();

    assert_eq!(
        without,
        vec![DENTALCIDIN_TUBE.slug()],
        "the set of product pages with no Supplement Facts panel changed"
    );
}

/// `servings_per_container` is optional, and four of #8's twelve captures prove
/// it — for two different reasons, only one of which is the page's own doing.
///
/// This is the assertion the old sweep made that could not have failed: eight
/// pages, all of which happened to state the row, all of which happened to
/// spell it the one way the parser matches.
#[test]
fn servings_per_container_is_absent_on_four_pages_for_two_different_reasons() {
    let without: Vec<&str> = fixture::products()
        .filter_map(|f| parse_supplement_facts_html(&f.doc()).map(|facts| (f, facts)))
        .filter(|(_, facts)| facts.servings_per_container.is_none())
        .map(|(f, _)| f.slug())
        .collect();

    assert_eq!(
        without,
        vec![
            LITHIUM_MICRO_TABLETS.slug(),
            SUPREME_C_TABLETS.slug(),
            BUTYRATE_TWO_CAP_SERVING.slug(),
            R_LIPOIC_TINY_ID.slug(),
        ],
        "the set of pages without a servings-per-container reading changed; if \
         the parser was taught the singular spelling, the last two rows are the \
         ones that should have left"
    );

    // Reason one: the page never says it. Neither of these two pages contains
    // the phrase in any spelling, so `None` is the whole truth about them.
    for f in [LITHIUM_MICRO_TABLETS, SUPREME_C_TABLETS] {
        assert!(
            !f.html().to_lowercase().contains("per container"),
            "{} does state a per-container count after all",
            f.slug()
        );
    }

    // Reason two, and the one that is ours: these two pages *do* state it, and
    // spell it "Serving Per Container" — singular. `parse_supplement_facts_html`
    // matches `"servings per"`, so it reads past them and answers `None` for a
    // page that gave an answer.
    //
    // Not fixed here. This commit adds fixtures; the gap it exposes is parser
    // work, and a production change buried in a fixture commit is a change
    // nobody reviews. Filed as #54, and this test is what will go red when it
    // is fixed.
    for f in [BUTYRATE_TWO_CAP_SERVING, R_LIPOIC_TINY_ID] {
        assert!(
            f.html().contains("Serving Per Container"),
            "{} was supposed to be a singular-spelling page",
            f.slug()
        );
    }
}

/// `Package quantity` is not a count, and #15 has to know that before it
/// designs a quantity type.
///
/// The eight pages this suite had before #8 offered two shapes — `"<n> count"`
/// on seven and `"8.81 oz"` on the powder — so "a number and a unit noun" was
/// an unfalsifiable reading of the field. The current corpus carries five
/// shapes, including a **bare number with no unit at all** and the suite's
/// first **volume**.
#[test]
fn package_quantity_takes_five_shapes_and_only_one_of_them_is_a_count() {
    let quantity = |f: fixture::Fixture| extract_spec(&f.doc(), "Package quantity");

    // A count of units. What every US capture but one says, and what a
    // quantity model built on this corpus alone would have assumed.
    assert_eq!(quantity(TWO_A_DAY).as_deref(), Some("60 count"));

    // Imperial mass.
    assert_eq!(quantity(GOLD_C_POWDER).as_deref(), Some("8.81 oz"));

    // Metric mass — and note it disagrees with the *volume* the tube is sold
    // by. The registry buys this as a 90 ml tube; iHerb's package quantity
    // calls it 85 g. Neither is wrong and they are not convertible without a
    // density, which is the kind of thing #15 has to decide rather than paper
    // over.
    assert_eq!(quantity(DENTALCIDIN_TUBE).as_deref(), Some("85 g"));

    // Volume. The corpus had none before #8.
    assert_eq!(quantity(TART_CHERRY_LIQUID).as_deref(), Some("946 ml"));

    // A bare number. No unit, no noun — the page states `250` for a product
    // sold as 250 grams of powder, and nothing on the field says which.
    assert_eq!(quantity(FIBERAID_POWDER).as_deref(), Some("250"));
}

/// `Dimensions` is published in the storefront's own unit system, and the two
/// systems are not distinguishable by shape alone — both are three numbers, an
/// `x` separator and a mass.
///
/// Held constant against the product: this is the same Nordic Naturals bottle
/// on two storefronts, so the only thing that differs is who is describing it.
#[test]
fn dimensions_carry_the_storefronts_units_not_one_systems() {
    assert_eq!(
        extract_spec(&ULTIMATE_OMEGA.doc(), "Dimensions").as_deref(),
        Some("5.85 x 3.2 x 3.15 in, 0.72 lb")
    );
    assert_eq!(
        extract_spec(&ULTIMATE_OMEGA_NOK.doc(), "Dimensions").as_deref(),
        Some("14.9 x 8.1 x 8 cm, 0.33 kg")
    );
}

/// Three shapes the table takes: a 29-row multivitamin, a single-nutrient
/// powder, and a gummy whose first row is a calorie count with no daily value.
#[test]
fn supplement_facts_keep_row_order_and_daily_values() {
    let multi = parse_supplement_facts_html(&TWO_A_DAY.doc()).unwrap();
    assert_eq!(multi.serving_size.as_deref(), Some("2 Capsules"));
    assert_eq!(multi.servings_per_container.as_deref(), Some("30"));
    assert_eq!(multi.nutrients.len(), 29);
    assert_eq!(
        multi.nutrients[0].name,
        "Vitamin A (as Retinyl Acetate and 50% as Beta-Carotene)"
    );
    assert_eq!(multi.nutrients[0].amount, "1500 mcg");
    assert_eq!(multi.nutrients[0].daily_value.as_deref(), Some("167%"));

    let powder = parse_supplement_facts_html(&GOLD_C_POWDER.doc()).unwrap();
    assert_eq!(powder.serving_size.as_deref(), Some("1 Scoop (1 g)"));
    assert_eq!(powder.servings_per_container.as_deref(), Some("250"));
    assert_eq!(powder.nutrients.len(), 1);
    assert_eq!(powder.nutrients[0].name, "Vitamin C (as Ascorbic Acid)");
    assert_eq!(powder.nutrients[0].amount, "1,000 mg");

    let gummies = parse_supplement_facts_html(&OLLY_GUMMIES.doc()).unwrap();
    assert_eq!(gummies.serving_size.as_deref(), Some("2 Gummies"));
    assert_eq!(gummies.nutrients[0].name, "Calories");
    assert_eq!(gummies.nutrients[0].amount, "15");
    assert_eq!(gummies.nutrients[0].daily_value, None);
}

#[test]
fn supplement_facts_are_none_without_a_table() {
    assert!(parse_supplement_facts_html(&fixture::empty_doc()).is_none());
    // The search results page has product cards but no supplement table.
    assert!(parse_supplement_facts_html(&fixture::SEARCH_VITAMIN_C.doc()).is_none());
}

// ---------------------------------------------------------------------------
// parse_review_distribution_html
// ---------------------------------------------------------------------------

/// How many `button.item` bars the page's review-histogram widget actually
/// holds. Zero covers both "empty shell" and "no widget"; the two are told
/// apart by whether the element is in the HTML at all.
fn populated_bars(f: crate::fixture::Fixture) -> usize {
    let doc = f.doc();
    let sel = scraper::Selector::parse("ugc-review-progress-bar button.item").unwrap();
    doc.select(&sel).count()
}

fn read_histogram(html: &str) -> HistogramRead {
    parse_review_distribution_html(&scraper::Html::parse_document(html))
}

/// **State 3 of 4: hydrated, and read.** #32, flipped. This was
/// `review_distribution_is_never_found_on_a_real_page`.
///
/// The five captures fall into three groups, and only the first was ever
/// evidence of the bug:
///
/// - **product-104996 alone** carries a populated `<ugc-review-progress-bar>`:
///   five `button.item` bars, five `each-count` spans, five `width: N%` values.
///   The parser returned nothing on it, and that was #32.
/// - **product-108255 and product-59561** carry the element as an empty
///   68-byte shell with no buttons at all — the widget had not filled in when
///   the page was captured. `NotHydrated` is the answer there.
/// - **product-119174 and product-12949** have no such element anywhere.
///   `Absent` is the answer there.
///
/// So one page, not three, ever proved the bug, and one page is what this
/// asserts. The parser identified a bar's star level by looking for the words
/// `"5 stars"` in the button's text; the real markup draws the level as an
/// `<ugc-star>` full of SVG with no such text, so every bar was skipped.
///
/// The bar counts are asserted per fixture on purpose. If a later re-capture
/// fills in the two empty shells, this test says so rather than letting the
/// one-page claim quietly widen into a five-page one.
#[test]
fn review_distribution_is_read_on_the_one_page_that_has_one() {
    assert_eq!(
        parse_review_distribution_html(&TWO_A_DAY.doc()),
        HistogramRead::Read(ReviewDistribution {
            five_star: Some(79.0),
            four_star: Some(14.0),
            three_star: Some(5.0),
            two_star: Some(1.0),
            one_star: Some(1.0),
        })
    );

    // The bars are read as percentages, and the widget's own `each-count`
    // spans say what they are percentages of: 10,434 + 1,865 + 645 + 138 + 113
    // is 13,195, the page's review count. That the level came from the right
    // bar is not an assumption about DOM order — the arithmetic agrees.

    for (f, bars) in [
        (TWO_A_DAY, 5),
        (B_COMPLEX, 0),
        (GOLD_C_POWDER, 0),
        (OLLY_GUMMIES, 0),
        (ULTIMATE_OMEGA, 0),
    ] {
        assert_eq!(populated_bars(f), bars, "{}", f.slug());
    }
}

/// **States 1 and 2 of 4: no widget, and an unhydrated shell.**
///
/// Both are the page having no histogram, and both must stay clear of
/// `Malformed`: neither is a failure of ours. They are still two different
/// answers, and the parser keeps them apart.
#[test]
fn an_absent_widget_and_an_empty_shell_are_two_different_absences() {
    for f in [OLLY_GUMMIES, ULTIMATE_OMEGA] {
        assert_eq!(
            parse_review_distribution_html(&f.doc()),
            HistogramRead::Absent,
            "{} has no widget element at all",
            f.slug()
        );
        assert!(
            !f.html().contains("<ugc-review-progress-bar"),
            "{}",
            f.slug()
        );
    }

    for f in [B_COMPLEX, GOLD_C_POWDER] {
        assert_eq!(
            parse_review_distribution_html(&f.doc()),
            HistogramRead::NotHydrated,
            "{} carries the widget as an empty shell",
            f.slug()
        );
        assert!(
            f.html().contains("<ugc-review-progress-bar"),
            "{}",
            f.slug()
        );
    }

    assert_eq!(
        parse_review_distribution_html(&fixture::empty_doc()),
        HistogramRead::Absent
    );
    assert_eq!(
        read_histogram("<ugc-review-progress-bar></ugc-review-progress-bar>"),
        HistogramRead::NotHydrated
    );
}

/// **State 4 of 4: hydrated, and unreadable.** The state that did not exist
/// before this round, and the reason the return type is no longer an `Option`.
///
/// Every marker the parser reads is drawn rather than declared — iHerb gives
/// the buttons no aria label, no data attribute and no per-level class — so
/// every marker can rot. When one does, the bars are still *there*, and saying
/// "this product has no histogram" would be a lie told quietly. All three
/// shapes of that failure report themselves.
#[test]
fn a_hydrated_widget_it_cannot_read_is_malformed_not_absent() {
    // The glyph reading rots: stars redrawn so none of them reads as filled.
    // Every button lands on no level at all.
    assert_eq!(
        read_histogram(
            r#"<ugc-review-progress-bar>
                 <button class="item"><ugc-star><ul>
                   <li class="ugc-star-item"><svg><path fill="white"></path></svg></li>
                 </ul></ugc-star>
                 <div class="percent-wrap"><span class="block" style="width: 84%;"></span></div>
                 </button>
               </ugc-review-progress-bar>"#
        ),
        HistogramRead::Malformed(HistogramFault::NoBarNamesItsLevel)
    );

    // The glyph reading rots the other way: every star reads as filled, so
    // every button claims five stars. This is the guard round 1 added, and the
    // point of this round is that it no longer reports plain absence.
    assert_eq!(
        read_histogram(
            r#"<ugc-review-progress-bar>
                 <button class="item"><span>5 stars</span>
                   <div class="percent-wrap"><span class="block" style="width: 84%;"></span></div>
                 </button>
                 <button class="item"><span>5 stars</span>
                   <div class="percent-wrap"><span class="block" style="width: 1%;"></span></div>
                 </button>
               </ugc-review-progress-bar>"#
        ),
        HistogramRead::Malformed(HistogramFault::DuplicateLevel)
    );

    // The bar markup rots instead of the glyph: levels resolve, widths do not.
    assert_eq!(
        read_histogram(
            r#"<ugc-review-progress-bar>
                 <button class="item"><span>5 stars</span>
                   <div class="bar-wrap"><i data-width="84"></i></div></button>
                 <button class="item"><span>4 stars</span>
                   <div class="bar-wrap"><i data-width="10"></i></div></button>
               </ugc-review-progress-bar>"#
        ),
        HistogramRead::Malformed(HistogramFault::NoBarCarriesAWidth)
    );
}

/// A malformed histogram reaches `ExtractionHealth`, which is the half of this
/// that a caller can act on.
///
/// The record carries no distribution — there is nothing to carry, and
/// inventing a bar is the bug this codebase exists to prevent — but
/// `review_distribution` is `Malformed`, it is listed in `fields_malformed`,
/// and `degraded` is true even though `review_distribution` is deliberately not
/// in `EXPECTED_FIELDS`.
#[test]
fn a_malformed_histogram_degrades_the_record() {
    // The gummies page, given a histogram whose bars cannot be told apart.
    // The page carries no widget of its own, which is what makes it the right
    // page to graft a broken one onto: the only difference from the intact run
    // below is the widget.
    const BROKEN_WIDGET: &str = r#"<ugc-review-progress-bar>
          <button class="item"><span>5 stars</span>
            <div class="percent-wrap"><span class="block" style="width: 84%;"></span></div></button>
          <button class="item"><span>5 stars</span>
            <div class="percent-wrap"><span class="block" style="width: 1%;"></span></div></button>
        </ugc-review-progress-bar><ul id="product-specs-list""#;
    let broken = OLLY_GUMMIES
        .html()
        .replace(r#"<ul id="product-specs-list""#, BROKEN_WIDGET);
    assert_ne!(broken, OLLY_GUMMIES.html(), "the graft must have taken");
    let mut product = parse_from_json_ld(&OLLY_GUMMIES.json_ld(), "119174", BASE_URL).unwrap();
    enrich_from_html(&broken, &mut product);
    let health = product.health();

    assert_eq!(product.review_distribution, None, "nothing may be invented");
    assert_eq!(product.source_of("review_distribution"), Source::Malformed);
    assert!(health
        .fields_malformed
        .contains(&"review_distribution".to_string()));
    assert!(
        !health
            .fields_absent
            .contains(&"review_distribution".to_string()),
        "malformed is not absent — that conflation is the whole bug"
    );
    assert!(
        !iherb_cli::model::ProductDetail::EXPECTED_FIELDS.contains(&"review_distribution"),
        "the point is that a malformed field degrades without being expected"
    );
    assert!(health.degraded, "a widget we could not read is rot");

    // And the same page untouched is not degraded: no widget, no complaint.
    let mut intact = parse_from_json_ld(&OLLY_GUMMIES.json_ld(), "119174", BASE_URL).unwrap();
    enrich_from_html(OLLY_GUMMIES.html(), &mut intact);
    assert_eq!(
        intact.source_of("review_distribution"),
        iherb_cli::model::Source::Absent
    );
    assert!(!intact.health().degraded);
}

/// The star level a hydrated bar stands for is drawn, not written, and what is
/// read off the drawing is now its *structure* rather than its colour.
///
/// iHerb draws an empty star as a ground layer plus an outline and a filled one
/// by inserting a fill layer between them, so a filled star carries two painted
/// `<path>`s and an empty one carries one. Keying on `#FAC627` instead — which
/// is what #32 shipped — meant a re-theme would silently empty the histogram,
/// and the fixed fixtures would not notice until someone re-captured.
///
/// This asserts the reading directly rather than only through the distribution,
/// and asserts that the two readings agree on this page: colour and structure
/// both say five, four, three, two, one.
#[test]
fn a_hydrated_bar_names_its_star_level_in_filled_glyphs() {
    let doc = TWO_A_DAY.doc();
    let button_sel = scraper::Selector::parse("ugc-review-progress-bar button.item").unwrap();
    let star_sel = scraper::Selector::parse("li.ugc-star-item").unwrap();
    let painted_sel = scraper::Selector::parse("path[fill]").unwrap();
    let gold_sel = scraper::Selector::parse(r##"li.ugc-star-item path[fill="#FAC627"]"##).unwrap();

    let by_structure: Vec<usize> = doc
        .select(&button_sel)
        .map(|b| {
            b.select(&star_sel)
                .filter(|star| {
                    star.select(&painted_sel)
                        .filter(|p| p.value().attr("fill") != Some("none"))
                        .count()
                        >= 2
                })
                .count()
        })
        .collect();
    assert_eq!(by_structure, [5, 4, 3, 2, 1]);

    // The colour the old reading used says the same thing on this page, which
    // is why swapping to structure changed no value — only what can rot.
    let by_colour: Vec<usize> = doc
        .select(&button_sel)
        .map(|b| b.select(&gold_sel).count())
        .collect();
    assert_eq!(by_colour, by_structure);

    // And no bar says so in words, which is what the parser looked for before
    // #32 and still looks for first.
    for button in doc.select(&button_sel) {
        let text: String = button.text().collect();
        assert!(
            !text.contains("star"),
            "the hydrated widget writes no star-label text: {:?}",
            text
        );
    }
}

/// A re-theme of the star colour must not empty the histogram.
///
/// This is Finding 1 stated as a test: the same capture with every `#FAC627`
/// repainted still reads five, four, three, two, one, because the structure the
/// parser reads did not move. Against the colour-keyed version this page came
/// back as five buttons claiming no level at all.
#[test]
fn a_restyled_star_colour_does_not_empty_the_histogram() {
    let retheme = TWO_A_DAY.html().replace("#FAC627", "#00B67A");
    assert_eq!(
        read_histogram(&retheme),
        parse_review_distribution_html(&TWO_A_DAY.doc()),
        "the histogram must not depend on the brand's gold"
    );
    assert!(matches!(read_histogram(&retheme), HistogramRead::Read(_)));
}

/// The other shape the parser reads: a `<span>` naming the star level in words,
/// and a bar whose width carries the percentage.
///
/// No capture renders this, so it is asserted against hand-written markup. It
/// is kept rather than replaced by the glyph reading because only one captured
/// page is hydrated, and one sample is not enough to declare the written form
/// gone from every live page.
#[test]
fn review_distribution_parses_the_shape_the_parser_documents() {
    let html = r#"
        <ugc-review-progress-bar class="ugc-review-progress-wrap">
          <button class="item"><span>5 stars</span>
            <div class="percent-wrap"><span class="block" style="width: 84%;"></span></div>
            <span class="each-count">11091</span></button>
          <button class="item"><span>4 stars</span>
            <div class="percent-wrap"><span class="block" style="width: 10%;"></span></div></button>
          <button class="item"><span>3 stars</span>
            <div class="percent-wrap"><span class="block" style="width: 3%;"></span></div></button>
          <button class="item"><span>2 stars</span>
            <div class="percent-wrap"><span class="block" style="width: 2%;"></span></div></button>
          <button class="item"><span>1 star</span>
            <div class="percent-wrap"><span class="block" style="width: 1%;"></span></div></button>
        </ugc-review-progress-bar>
    "#;
    assert_eq!(
        read_histogram(html),
        HistogramRead::Read(ReviewDistribution {
            five_star: Some(84.0),
            four_star: Some(10.0),
            three_star: Some(3.0),
            two_star: Some(2.0),
            one_star: Some(1.0),
        })
    );
}

/// A widget that yields *some* bars is a read, not a failure.
///
/// A star level with no reviews may legitimately have no bar, and `None` in a
/// bucket already means "unknown". Calling a three-bar widget malformed would
/// trade the silence this round is fixing for a false alarm.
#[test]
fn a_partly_filled_widget_is_read_with_the_rest_unknown() {
    let html = r#"
        <ugc-review-progress-bar>
          <button class="item"><span>5 stars</span>
            <div class="percent-wrap"><span class="block" style="width: 90%;"></span></div></button>
          <button class="item"><span>4 stars</span>
            <div class="percent-wrap"><span class="block" style="width: 10%;"></span></div></button>
        </ugc-review-progress-bar>
    "#;
    assert_eq!(
        read_histogram(html),
        HistogramRead::Read(ReviewDistribution {
            five_star: Some(90.0),
            four_star: Some(10.0),
            three_star: None,
            two_star: None,
            one_star: None,
        })
    );
}
