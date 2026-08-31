//! Proves acceptance criterion 1 of #29: `cargo test` can exercise library code
//! from `tests/` without invoking the binary. #8 builds the real fixture suite
//! on top of this.

use iherb_cli::cache::CacheKey;
use iherb_cli::cli::SortOrder;
use iherb_cli::config::AppConfig;
use iherb_cli::fetch::FetchTarget;
use iherb_cli::model::{ProductSummary, SearchResult};
use iherb_cli::output::format_search_results;
use iherb_cli::scraper::search::{build_search_url, pages_needed};
use iherb_cli::targets::product::parse_product_identifier;
use iherb_cli::targets::{ProductTarget, SearchTarget};

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

fn test_config() -> AppConfig {
    AppConfig {
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

#[test]
fn a_command_is_a_target_descriptor() {
    let config = test_config();

    let product = ProductTarget::new(&config, "102110").unwrap();
    assert_eq!(product.url(1), "https://www.iherb.com/pr/item/102110");
    assert_eq!(product.page_count(), 1);
    assert_eq!(
        product.cache_key(),
        CacheKey::Product {
            product_id: "102110".to_string()
        }
    );

    let search = SearchTarget::new(&config, "vitamin c", 100, SortOrder::Relevance, None).unwrap();
    assert!(search.page_count() > 1);
    assert_ne!(search.url(1), search.url(2));
    assert_eq!(
        search.cache_key(),
        CacheKey::Search {
            query: "vitamin c".to_string(),
            sort: SortOrder::Relevance,
            category: None,
        }
    );

    // Input validation happens when the target is built, before any cache
    // lookup or browser launch.
    assert!(SearchTarget::new(&config, "   ", 20, SortOrder::Relevance, None).is_err());
    assert!(SearchTarget::new(&config, "vitamin c", 0, SortOrder::Relevance, None).is_err());
    assert!(ProductTarget::new(&config, "not-a-product").is_err());
}
