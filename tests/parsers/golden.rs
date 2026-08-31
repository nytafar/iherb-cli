//! Golden-output tests for the Markdown formatter.
//!
//! The CLI's whole output is Markdown an agent reads, so a formatting change is
//! a contract change. These render a captured page end to end and diff against
//! a checked-in file; run `UPDATE_GOLDEN=1 cargo test` to rewrite the goldens,
//! then read the diff before committing it.
//!
//! The goldens are characterizations like everything else here, and two known
//! bugs are visible in them. `## Overview` has no `Shipping Weight` line,
//! because #2 loses it; and `## Reviews` never appears at all, because
//! `parse_review_distribution_html` finds nothing on a real page (see
//! `product_dom::review_distribution_is_never_found_on_a_real_page`). When
//! either lands, the golden grows a line and that diff is the proof.

use iherb_cli::cli::Section;
use iherb_cli::output::{format_product_detail, format_search_results};
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
/// no warnings, no review histogram, and an out-of-stock line.
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
