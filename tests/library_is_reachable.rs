//! Proves acceptance criterion 1 of #29: `cargo test` can exercise library code
//! from `tests/` without invoking the binary. #8 builds the real fixture suite
//! on top of this.

use iherb_cli::app::parse_product_identifier;
use iherb_cli::cli::SortOrder;
use iherb_cli::config::AppConfig;
use iherb_cli::model::{ProductSummary, SearchResult};
use iherb_cli::output::format_search_results;
use iherb_cli::scraper::search::{build_search_url, pages_needed};

#[test]
fn product_identifier_accepts_an_id_and_a_url() {
    assert_eq!(parse_product_identifier("102110").unwrap(), "102110");
    assert_eq!(
        parse_product_identifier("https://www.iherb.com/pr/some-slug/102110").unwrap(),
        "102110"
    );
    assert!(parse_product_identifier("not-a-product").is_err());
}

#[test]
fn config_validates_country_codes() {
    assert!(AppConfig::validate_country("us").is_ok());
    assert!(AppConfig::validate_country("no").is_ok());
    assert!(AppConfig::validate_country("zz").is_err());
}

#[test]
fn search_urls_and_paging_are_derived_from_the_limit() {
    assert_eq!(pages_needed(1), 1);
    assert!(pages_needed(100) > 1);

    let url = build_search_url(
        "https://www.iherb.com",
        "vitamin c",
        SortOrder::PriceAsc,
        None,
        1,
    );
    assert!(url.starts_with("https://www.iherb.com"));
    assert!(url.contains(SortOrder::PriceAsc.as_url_param()));
}

#[test]
fn search_results_render_as_markdown() {
    let result = SearchResult {
        query: "vitamin c".to_string(),
        total_results: Some(1),
        products: vec![ProductSummary {
            name: "Acme, Vitamin C, 60 Capsules".to_string(),
            brand: "Acme".to_string(),
            price: 12.34,
            original_price: None,
            currency: "USD".to_string(),
            rating: Some(4.5),
            review_count: Some(7),
            product_url: "https://www.iherb.com/pr/acme/1".to_string(),
            product_id: "1".to_string(),
            in_stock: true,
        }],
    };

    let rendered = format_search_results(&result);
    assert!(rendered.contains("vitamin c"));
    assert!(rendered.contains("Acme, Vitamin C, 60 Capsules"));
    assert!(rendered.contains("**ID:** 1"));
}
