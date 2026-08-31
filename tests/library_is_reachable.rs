//! Proves acceptance criterion 1 of #29: `cargo test` can exercise library code
//! from `tests/` without invoking the binary. #8 builds the real fixture suite
//! on top of this.

use iherb_cli::browser::session::BrowserSession;
use iherb_cli::cache::CacheKey;
use iherb_cli::cli::SortOrder;
use iherb_cli::config::AppConfig;
use iherb_cli::error::IherbError;
use iherb_cli::fetch::fetch_on;
use iherb_cli::fetch::FetchTarget;
use iherb_cli::model::{ProductDetail, ProductSummary, SearchResult, Source, Strategy};
use iherb_cli::output::format_search_results;
use iherb_cli::scraper::search::{build_search_url, page_budget};
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
    assert!(page_budget(1) >= 1);
    assert!(page_budget(100) > 1);

    let url = build_search_url(
        "https://www.iherb.com",
        "vitamin c",
        SortOrder::PriceAsc,
        None,
        1,
    );
    assert!(url.starts_with("https://www.iherb.com"));
    assert!(url.contains(&SortOrder::PriceAsc.as_url_param()));
}

#[test]
fn search_results_render_as_markdown() {
    let result = SearchResult {
        query: "vitamin c".to_string(),
        total_results: Some(1),
        products: vec![ProductSummary {
            name: "Acme, Vitamin C, 60 Capsules".to_string(),
            brand: "Acme".to_string(),
            price: Some(12.34),
            original_price: None,
            currency: "USD".to_string(),
            rating: Some(4.5),
            review_count: Some(7),
            product_url: "https://www.iherb.com/pr/acme/1".to_string(),
            product_id: "1".to_string(),
            in_stock: Some(true),
            extraction: Default::default(),
        }],
        fetch: Default::default(),
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
            country: "us".to_string(),
            product_id: "102110".to_string()
        }
    );

    let search = SearchTarget::new(&config, "vitamin c", 100, SortOrder::Relevance, None).unwrap();
    assert!(search.page_count() > 1);
    assert_ne!(search.url(1), search.url(2));
    assert_eq!(
        search.cache_key(),
        CacheKey::Search {
            country: "us".to_string(),
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

/// SF-2's whole point: `fetch_on` takes `&BrowserSession`, so #10 can drive
/// several against one session concurrently. That only works if the future is
/// `Send`, which is also why `FetchTarget::extract` declares one. Checked at
/// compile time, so a later change that makes it non-`Send` fails the build
/// rather than #10.
#[test]
fn fetch_on_futures_are_send_and_share_one_session() {
    fn assert_send<F: Send>(_: F) {}

    fn concurrent(config: &AppConfig, session: &BrowserSession) {
        let a = ProductTarget::new(config, "102110").unwrap();
        let b = ProductTarget::new(config, "858").unwrap();
        // Both borrow the same session at the same time, and both are Send.
        assert_send(futures::future::join(
            fetch_on(&a, config, session),
            fetch_on(&b, config, session),
        ));
    }

    // Naming it is the whole test: the body above had to typecheck to get here.
    let _: fn(&AppConfig, &BrowserSession) = concurrent;
}

/// #28's third requirement, at the layer that decides what a caller is told.
///
/// `ProductTarget::validate` used to ask whether the values looked like junk —
/// empty name, or a zero price with no rating and no review count — and call
/// anything that failed it `product_not_found`. That is why a Cloudflare block
/// reported as "product not found" (#23): the worst available answer, because
/// it tells the caller to give up on an id that is perfectly valid.
///
/// It now asks a provenance question instead: did any strategy produce a name
/// and a price? A record where they are `Absent` is `parse_failed`.
#[test]
fn a_record_no_strategy_produced_is_parse_failed_not_not_found() {
    let config = test_config();
    let target = ProductTarget::new(&config, "61864").unwrap();

    // What a blocked or structurally-changed page leaves behind: a record with
    // nothing attributed to any strategy.
    let nothing = bare_product();
    let err = target
        .validate(&nothing)
        .expect_err("a record nothing produced must be rejected");
    let err = err
        .downcast::<IherbError>()
        .expect("rejection must be a typed IherbError, not a string");
    assert!(
        matches!(err, IherbError::ParseFailed(ref id) if id == "61864"),
        "expected ParseFailed, got {:?}",
        err
    );

    // The old heuristic's exact false positive: a real product with a zero
    // price, no rating and no review count. Every one of those is legitimately
    // possible, and the old check called the whole page missing.
    let mut free_and_unreviewed = bare_product();
    free_and_unreviewed.extraction.strategy = Strategy::JsonLd;
    free_and_unreviewed.extraction.claim("name", Source::JsonLd);
    free_and_unreviewed
        .extraction
        .claim("price", Source::JsonLd);
    assert!(
        target.validate(&free_and_unreviewed).is_ok(),
        "a product whose fields were produced is valid, whatever the values are"
    );
}

/// `degraded` is a warning, not a rejection: the record is usable and suspect
/// at the same time, and throwing it away would lose data a caller can use.
#[test]
fn a_degraded_record_is_still_returned() {
    let config = test_config();
    let target = ProductTarget::new(&config, "61864").unwrap();

    let mut degraded = bare_product();
    degraded.extraction.strategy = Strategy::Dom;
    degraded.extraction.claim("name", Source::Dom);
    degraded.extraction.claim("price", Source::Dom);

    assert!(degraded.health().degraded, "brand and upc are absent");
    assert!(target.validate(&degraded).is_ok());
}

fn bare_product() -> ProductDetail {
    ProductDetail {
        name: "California Gold Nutrition, Gold C".to_string(),
        brand: String::new(),
        price: 0.0,
        original_price: None,
        currency: "USD".to_string(),
        rating: None,
        review_count: None,
        product_url: "https://www.iherb.com/pr/p/61864".to_string(),
        product_id: "61864".to_string(),
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
}
