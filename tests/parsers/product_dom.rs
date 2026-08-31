//! The parsers fed page HTML or a parsed document: `parse_from_html`,
//! `enrich_from_html`, `extract_spec`, `parse_supplement_facts_html`,
//! `parse_review_distribution_html`.

use iherb_cli::scraper::helpers::is_not_found_page;
use iherb_cli::scraper::product::{
    enrich_from_html, extract_spec, parse_from_html, parse_from_json_ld,
    parse_review_distribution_html, parse_supplement_facts_html,
};

use crate::fixture::{
    self, BASE_URL, B_COMPLEX, GOLD_C_POWDER, OLLY_GUMMIES, TWO_A_DAY, ULTIMATE_OMEGA,
};

// ---------------------------------------------------------------------------
// parse_from_html — the last-resort DOM fallback
// ---------------------------------------------------------------------------

#[test]
fn dom_fallback_reads_every_product_page() {
    for f in fixture::products() {
        let product = parse_from_html(f.html(), f.product_id(), BASE_URL, "USD")
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
        let dom = parse_from_html(f.html(), f.product_id(), BASE_URL, "USD").unwrap();
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

/// CHARACTERIZATION, NOT DESIRED: pins the #2 bug from the outside. The DOM
/// fallback loses `product_code` and `shipping_weight` on every page, because
/// `extract_spec` is asked for `"Product Code"` / `"Shipping Weight"` and the
/// page says `Product code:` / `Shipping weight:`. `UPC` survives only because
/// it is an initialism and the case happens to match.
///
/// #2 flips this: after it lands, `product_code` and `shipping_weight` are
/// `Some`. Do not "fix" the code to match this test; fix the test when #2 lands.
#[test]
fn dom_fallback_loses_product_code_and_shipping_weight() {
    for f in fixture::products() {
        let product = parse_from_html(f.html(), f.product_id(), BASE_URL, "USD").unwrap();
        assert_eq!(product.product_code, None, "{}", f.slug());
        assert_eq!(product.shipping_weight, None, "{}", f.slug());
        assert!(product.upc.is_some(), "{}", f.slug());
    }
}

/// CHARACTERIZATION, NOT DESIRED: pins #31. The gummies page is out of stock —
/// JSON-LD says `OutOfStock` and `json_ld_reads_out_of_stock` asserts it — but the DOM
/// fallback reports it in stock. `#stock-status .stock-status-content strong`
/// finds nothing, and the fallback is `!html.contains("Out of Stock")`, which
/// the page satisfies because it never uses that exact string.
///
/// This is #31. It flips this to `assert!(!product.in_stock)`.
#[test]
fn dom_fallback_reports_the_gummies_as_in_stock() {
    let product = parse_from_html(OLLY_GUMMIES.html(), "119174", BASE_URL, "USD").unwrap();
    assert!(product.in_stock);
    assert!(!OLLY_GUMMIES.html().contains("Out of Stock"));
}

/// CHARACTERIZATION, NOT DESIRED: pins #5 from the parser side. The `currency`
/// argument is a label of last resort, never a request parameter: the captured
/// US pages carry `USD` and it wins, so `--currency CHF` silently produces USD
/// prices labelled USD. The danger case is the opposite one — see
/// `helpers::currency_detection_falls_through_on_a_page_with_no_markers`.
#[test]
fn dom_fallback_ignores_the_requested_currency() {
    for f in fixture::products() {
        let product = parse_from_html(f.html(), f.product_id(), BASE_URL, "CHF").unwrap();
        assert_eq!(product.currency, "USD", "{}", f.slug());
    }
}

#[test]
fn dom_fallback_rejects_a_page_with_no_product() {
    let err = parse_from_html(
        "<html><body><p>nothing</p></body></html>",
        "1",
        BASE_URL,
        "USD",
    )
    .expect_err("a page with no h1 is not a product page");
    assert!(err.to_string().contains('1'));

    let err = parse_from_html("<html><title>404</title>", "42", BASE_URL, "USD")
        .expect_err("a 404 page is not a product page");
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
    assert!(
        !product.in_stock,
        "JSON-LD's OutOfStock must survive enrichment"
    );
}

// ---------------------------------------------------------------------------
// extract_spec — #product-specs-list
// ---------------------------------------------------------------------------

/// CHARACTERIZATION, NOT DESIRED: pins the #2 bug at the parser. The match is
/// `text.starts_with(label)`, so the label's case has to be exactly right.
///
/// Note for whoever fixes #2: lowercasing the comparison is necessary but not
/// sufficient. `Shipping weight` matches once the case is right, and then
/// returns the value with the whole info-tooltip glued to it, because the
/// tooltip lives inside the same `<li>` and the parser takes everything after
/// the first colon. The fix has to bound the value as well as the label.
#[test]
fn extract_spec_matches_labels_case_sensitively() {
    let doc = ULTIMATE_OMEGA.doc();

    // What production asks for.
    assert_eq!(extract_spec(&doc, "Product Code"), None);
    assert_eq!(extract_spec(&doc, "Shipping Weight"), None);
    assert_eq!(extract_spec(&doc, "UPC").as_deref(), Some("768990037900"));

    // What the page actually says.
    assert_eq!(
        extract_spec(&doc, "Product code").as_deref(),
        Some("NOR-03790")
    );
    let weight = extract_spec(&doc, "Shipping weight").expect("the page has one");
    assert!(weight.starts_with("0.72 lb"), "{:?}", weight);
    assert!(
        weight.contains("The Shipping Weight includes the product"),
        "the tooltip is still glued to the value: {:?}",
        weight
    );
}

/// Every capture has a `#product-specs-list`, the gummies page included — the
/// three labels below resolve on all five once the case is right. #2 also wants
/// `Package quantity` and `First available`, which are already reachable.
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
}

// ---------------------------------------------------------------------------
// parse_supplement_facts_html
// ---------------------------------------------------------------------------

#[test]
fn supplement_facts_parse_on_every_product_page() {
    for f in fixture::products() {
        let facts = parse_supplement_facts_html(&f.doc())
            .unwrap_or_else(|| panic!("{}: no supplement facts", f.slug()));
        assert!(!facts.nutrients.is_empty(), "{}", f.slug());
        assert!(facts.serving_size.is_some(), "{}", f.slug());
        assert!(facts.servings_per_container.is_some(), "{}", f.slug());
    }
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

/// CHARACTERIZATION, NOT DESIRED: pins #32. `parse_review_distribution_html`
/// returns `None` on all five pages, so `## Reviews` never renders a histogram.
///
/// The five captures fall into three groups, and only the first is evidence of
/// the bug:
///
/// - **product-104996 alone** carries a populated `<ugc-review-progress-bar>`:
///   five `button.item` bars, five `each-count` spans, five `width: N%` values.
///   The parser still returns `None`, and that is #32.
/// - **product-108255 and product-59561** carry the element as an empty
///   68-byte shell with no buttons at all — the widget had not filled in when
///   the page was captured. `None` is the correct answer there.
/// - **product-119174 and product-12949** have no such element anywhere.
///   `None` is correct there too.
///
/// So one page, not three, proves the bug. The parser identifies a bar's star
/// level by looking for the words `"5 stars"` in the button's text; the real
/// markup draws the stars as an `<ugc-star>` element full of SVG with no such
/// text, so every bar is skipped and the function bails.
///
/// #32 flips the 104996 case to a populated distribution and leaves the other
/// four `None`. `review_distribution_parses_the_shape_the_parser_documents`
/// below shows the arithmetic is fine once a star level can be found.
#[test]
fn review_distribution_is_never_found_on_a_real_page() {
    for f in fixture::products() {
        assert!(
            parse_review_distribution_html(&f.doc()).is_none(),
            "{} now yields a distribution — has the star-level lookup been fixed?",
            f.slug()
        );
    }

    // Exactly one page has data for the parser to lose. Counting the bars
    // keeps the three groups above honest: if a later re-capture fills in the
    // empty shells, this test says so rather than quietly widening its claim.
    for (f, bars) in [
        (TWO_A_DAY, 5),
        (B_COMPLEX, 0),
        (GOLD_C_POWDER, 0),
        (OLLY_GUMMIES, 0),
        (ULTIMATE_OMEGA, 0),
    ] {
        assert_eq!(populated_bars(f), bars, "{}", f.slug());
    }

    // The distinction the groups turn on: an empty shell is present-but-blank,
    // and the gummies page has no element at all.
    assert!(B_COMPLEX.html().contains("<ugc-review-progress-bar"));
    assert!(!OLLY_GUMMIES.html().contains("<ugc-review-progress-bar"));
}

/// The shape `parse_review_distribution_html`'s own doc comment describes: a
/// `<span>` naming the star level, and a bar whose width carries the percentage.
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
    let dist = parse_review_distribution_html(&scraper::Html::parse_document(html))
        .expect("the documented shape must parse");

    assert_eq!(dist.five_star, Some(84.0));
    assert_eq!(dist.four_star, Some(10.0));
    assert_eq!(dist.three_star, Some(3.0));
    assert_eq!(dist.two_star, Some(2.0));
    assert_eq!(dist.one_star, Some(1.0));
}

#[test]
fn review_distribution_is_none_without_the_widget() {
    assert!(parse_review_distribution_html(&fixture::empty_doc()).is_none());
    let empty_widget =
        scraper::Html::parse_document("<ugc-review-progress-bar></ugc-review-progress-bar>");
    assert!(parse_review_distribution_html(&empty_widget).is_none());
}
