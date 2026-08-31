//! Golden-output tests for the Markdown formatter.
//!
//! The CLI's whole output is Markdown an agent reads, so a formatting change is
//! a contract change. These render a captured page end to end and diff against
//! a checked-in file; run `UPDATE_GOLDEN=1 cargo test` to rewrite the goldens,
//! then read the diff before committing it.
//!
//! Both bugs the goldens used to characterize have landed, and each grew a
//! golden: `product-104996` gained the `Shipping Weight` line #2 ate and the
//! `## Reviews` histogram #32 could not read. Those diffs were the proof.
//!
//! `product-119174` still has neither section, and that is the page rather than
//! us: the gummies capture carries no `<ugc-review-progress-bar>` at all, so
//! `## Reviews` is genuinely absent and must not be rendered as zeroes.

use iherb_cli::cli::Section;
use iherb_cli::model::SearchFetch;
use iherb_cli::output::{format_product_detail, format_search_results, format_search_shortfall};
use iherb_cli::scraper::product::{enrich_from_html, parse_from_json_ld};
use iherb_cli::scraper::search::parse_search_from_html;

use crate::fixture::{assert_golden, BASE_URL, OLLY_GUMMIES, SEARCH_VITAMIN_C, TWO_A_DAY};

/// The path production takes for a product page: JSON-LD, then DOM enrichment.
fn as_production_would(f: crate::fixture::Fixture) -> iherb_cli::model::ProductDetail {
    let mut product = parse_from_json_ld(&f.json_ld(), f.product_id(), BASE_URL)
        .unwrap_or_else(|| panic!("{}: no JSON-LD", f.slug()));
    enrich_from_html(f.html(), &mut product);
    product
}

#[test]
fn product_renders_every_section() {
    let product = as_production_would(TWO_A_DAY);
    assert_golden(
        "product-104996-full",
        &format_product_detail(&product, None),
    );
}

/// The gummies page is missing most of what the formatter can print, so this
/// golden is the shape of a sparse product: no ingredients, no suggested use,
/// no warnings, no review histogram — the page has no widget at all — and an
/// out-of-stock line.
#[test]
fn product_renders_what_a_sparse_page_has() {
    let product = as_production_would(OLLY_GUMMIES);
    assert_golden(
        "product-119174-full",
        &format_product_detail(&product, None),
    );
}

#[test]
fn a_requested_section_renders_alone() {
    let product = as_production_would(TWO_A_DAY);
    assert_golden(
        "product-104996-nutrition",
        &format_product_detail(&product, Some(Section::Nutrition)),
    );
    assert_golden(
        "product-104996-overview",
        &format_product_detail(&product, Some(Section::Overview)),
    );
}

/// A section the page has no data for prints one honest line rather than an
/// empty heading.
#[test]
fn an_absent_section_says_so() {
    let product = as_production_would(OLLY_GUMMIES);
    assert_eq!(
        format_product_detail(&product, Some(Section::Warnings)),
        "No warnings data available for this product.\n"
    );
    assert_eq!(
        format_product_detail(&product, Some(Section::Reviews)),
        "No review data available for this product.\n"
    );
}

#[test]
fn search_results_render() {
    let mut result =
        parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL, "USD").unwrap();
    // `cmd_search` truncates to --limit before formatting; five is enough to
    // cover the separator, the discount line and the rating line.
    result.products.truncate(5);
    assert_golden("search-vitamin-c-top5", &format_search_results(&result));
}

/// A search that came up short of `--limit` says so, and says which kind of
/// short it is. `--limit` counts distinct products (#33), so a short result is
/// ordinary — which is exactly why it has to be stated: a caller counting rows
/// cannot otherwise tell "iHerb had no more" from "we stopped walking" (#6).
#[test]
fn a_search_short_of_the_limit_says_which_kind_of_short() {
    let mut result =
        parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL, "USD").unwrap();
    assert_eq!(result.products.len(), 45);

    // Asked for what we have: nothing to report.
    assert_eq!(format_search_shortfall(&result, 45), None);
    assert_eq!(format_search_shortfall(&result, 10), None);

    // Short because the walk stopped, with 11,952 results behind it.
    result.fetch = SearchFetch {
        pages_fetched: Some(1),
        exhausted: Some(false),
    };
    let note = format_search_shortfall(&result, 200).expect("45 is short of 200");
    assert!(
        note.contains("asked for 200, returning 45 distinct products"),
        "{}",
        note
    );
    assert!(note.contains("more behind these"), "{}", note);

    // Short because there is no more. Same count, opposite advice.
    result.fetch.exhausted = Some(true);
    let note = format_search_shortfall(&result, 200).expect("45 is short of 200");
    assert!(note.contains("iHerb had no more"), "{}", note);

    // A record that does not say is reported as not saying.
    result.fetch = SearchFetch::default();
    let note = format_search_shortfall(&result, 200).unwrap();
    assert!(note.contains("does not say"), "{}", note);
}

/// The `Data quality` line names the fields that were not read, whether they
/// are absent or merely defaulted. A record degraded purely by a defaulted
/// currency used to print an empty list, because the line only ever reported
/// absent fields.
#[test]
fn the_degraded_line_names_a_defaulted_field() {
    // A block complete except that the offers name no currency.
    let no_currency = serde_json::json!({
        "@type": "Product",
        "name": "Acme, Thing, 60 Capsules",
        "brand": { "name": "Acme" },
        "sku": "ACM-1",
        "gtin12": "000000000001",
        "offers": { "price": "9.60", "availability": "https://schema.org/InStock" },
    });
    let product = parse_from_json_ld(&no_currency, "1", BASE_URL).unwrap();
    let health = product.health();
    assert!(health.degraded);
    // Plenty of *unexpected* fields are absent — no ingredients, no warnings.
    // None of the EXPECTED ones is, so currency being defaulted is the only
    // thing making this record degraded, and it is the only thing the line has
    // to name. Reporting `fields_absent` here would print the wrong list.
    assert!(health.fields_defaulted.contains(&"currency".to_string()));
    for expected in iherb_cli::model::ProductDetail::EXPECTED_FIELDS {
        assert!(
            !health.fields_absent.contains(&expected.to_string()),
            "{} should have been read",
            expected
        );
    }

    let rendered = format_product_detail(&product, Some(Section::Overview));
    assert!(
        rendered.contains("- **Data quality:** degraded — no strategy produced currency."),
        "the line must name the field, not print an empty list: {:?}",
        rendered
    );
}

/// None of the captured pages is degraded on the production path, which is why
/// no golden carries a `Data quality` line. If one starts to, the goldens
/// change and this says why first.
#[test]
fn no_captured_page_is_degraded_on_the_production_path() {
    for f in crate::fixture::products() {
        let product = as_production_would(f);
        assert!(!product.health().degraded, "{}", f.slug());
    }
}

/// The `Data quality` line names only the fields that actually caused the
/// degradation, not every absent field on the page.
///
/// `degraded` is decided by `EXPECTED_FIELDS`, but the line used to print
/// `fields_absent`, which is every absent field there is. The gummies page on
/// the DOM path has several absent fields and only one of them is ever a reason
/// to call the record broken. Naming the innocents sends a reader hunting a
/// selector that is working fine, and is worse than saying nothing.
///
/// The degradation is manufactured, because no capture produces one any more:
/// #2 used to eat `product_code` off every page and this test borrowed that.
/// Relabelling the one spec row costs the DOM path `product_code` and nothing
/// else, which is the single-culprit shape the line is about.
#[test]
fn the_degraded_line_names_only_what_caused_the_degradation() {
    let relabelled = crate::fixture::OLLY_GUMMIES
        .html()
        .replace("Product code:", "Product identifier:");
    let product =
        iherb_cli::scraper::product::parse_from_html(&relabelled, "119174", BASE_URL, "USD")
            .unwrap();

    let health = product.health();
    assert!(health.degraded);
    // The innocents: absent, and none of them a reason to call anything broken.
    for innocent in ["ingredients", "suggested_use", "warnings", "original_price"] {
        assert!(
            health.fields_absent.contains(&innocent.to_string()),
            "{} should be absent on this page",
            innocent
        );
    }

    let line = format_product_detail(&product, Some(Section::Overview));
    assert!(
        line.contains("degraded — no strategy produced product_code."),
        "the line must name the culprit and only the culprit: {:?}",
        line
    );
    for innocent in ["ingredients", "suggested_use", "warnings", "original_price"] {
        assert!(
            !line.contains(innocent),
            "{} is absent but blameless, and must not appear in the degraded line: {:?}",
            innocent,
            line
        );
    }
}

/// A description that came from the `<meta name="description">` fallback is
/// marked as such. It is the full text cut to ~160 characters and it stops
/// mid-phrase, so printing it unmarked shows a reader a sentence that just ends
/// as though that were the product's description.
#[test]
fn a_truncated_description_says_it_is_truncated() {
    let via_dom =
        iherb_cli::scraper::product::parse_from_html(TWO_A_DAY.html(), "104996", BASE_URL, "USD")
            .unwrap();

    // The fallback really is what filled it, and it really does stop mid-phrase.
    assert_eq!(
        via_dom.source_of("description"),
        iherb_cli::model::Source::Dom
    );
    let desc = via_dom
        .description
        .clone()
        .expect("the page has a meta description");
    assert!(
        desc.ends_with("California Gold Nutrition® Multivitamin and"),
        "{:?}",
        desc
    );

    let rendered = format_product_detail(&via_dom, Some(Section::Description));
    assert!(rendered.contains(&desc), "the text itself is unchanged");
    assert!(
        rendered.contains("may stop mid-sentence"),
        "the truncation must be marked: {:?}",
        rendered
    );
    assert!(rendered.contains("#13"), "and point at who fixes it");

    // The JSON-LD description is the full one and carries no such note.
    let via_json_ld = as_production_would(TWO_A_DAY);
    assert_eq!(
        via_json_ld.source_of("description"),
        iherb_cli::model::Source::JsonLd
    );
    let rendered = format_product_detail(&via_json_ld, Some(Section::Description));
    assert!(
        !rendered.contains("may stop mid-sentence"),
        "{:?}",
        rendered
    );
    assert!(
        via_json_ld.description.unwrap().len() > desc.len(),
        "the structured-data description is the longer one"
    );
}

/// What a reader is told when the page carried a field extraction could not
/// read (#32 round 2).
///
/// **The input is synthetic, and deliberately so.** No captured page has a
/// malformed histogram — the one hydrated widget parses — so there is nothing
/// to characterize here and no golden covers this rendering. Rather than leave
/// it unprotected, the gummies page (which carries no widget of its own) is
/// grafted with a two-bar widget whose bars both claim five stars. That is the
/// same honesty the `next-data-*-synthetic` fixtures were labelled with in #8:
/// hand-written input, named as such, testing a path the captures cannot reach.
///
/// Three things this pins, all of which were wrong the moment `Source::Malformed`
/// existed and before `output.rs` caught up:
///
///  1. The `Data quality` line names the malformed field. It used to print
///     `no strategy produced .` — an empty list and a dangling full stop —
///     because it named only `EXPECTED_FIELDS`, and `review_distribution` is
///     deliberately not one.
///  2. It says the field *was on the page*, not that nothing produced it. Those
///     are different problems with different culprits.
///  3. `format_extraction_health` lists it under `Malformed`, beside the
///     existing `Absent` and `Defaulted` lines, and its `Degraded:` sentence no
///     longer claims a fact that is false here — every field every product page
///     publishes *was* read off this one.
#[test]
fn a_malformed_field_is_rendered_as_unreadable_not_as_missing() {
    // SYNTHETIC: hand-written widget grafted onto a real capture. See above.
    const BROKEN_WIDGET: &str = r#"<ugc-review-progress-bar>
          <button class="item"><span>5 stars</span>
            <div class="percent-wrap"><span class="block" style="width: 84%;"></span></div></button>
          <button class="item"><span>5 stars</span>
            <div class="percent-wrap"><span class="block" style="width: 1%;"></span></div></button>
        </ugc-review-progress-bar><ul id="product-specs-list""#;
    let grafted = OLLY_GUMMIES
        .html()
        .replace(r#"<ul id="product-specs-list""#, BROKEN_WIDGET);
    assert_ne!(grafted, OLLY_GUMMIES.html(), "the graft must have taken");

    let mut product = parse_from_json_ld(&OLLY_GUMMIES.json_ld(), "119174", BASE_URL).unwrap();
    enrich_from_html(&grafted, &mut product);

    let overview = format_product_detail(&product, Some(Section::Overview));
    assert!(
        overview.contains("degraded — review_distribution was on the page and could not be read."),
        "the line must name the field and say what went wrong: {:?}",
        overview
    );
    assert!(
        !overview.contains("no strategy produced ."),
        "the empty-list, dangling-period sentence must not come back: {:?}",
        overview
    );

    let health = iherb_cli::output::format_extraction_health(&product.health());
    assert!(
        health.contains("- **Malformed (on the page, unreadable):** review_distribution"),
        "{:?}",
        health
    );
    assert!(
        health.contains("a field the page carried could not be read"),
        "the Degraded sentence must cover this cause, not just the other one: {:?}",
        health
    );

    // The same page untouched says none of it: no widget, no complaint, and the
    // `Data quality` line stays absent entirely.
    let intact = as_production_would(OLLY_GUMMIES);
    assert!(!intact.health().degraded);
    assert!(!format_product_detail(&intact, Some(Section::Overview)).contains("Data quality"));
}
