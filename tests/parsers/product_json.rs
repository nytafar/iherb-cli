//! The parsers fed JSON rather than page HTML: `parse_from_json_ld` and
//! `parse_from_js_globals`.
//!
//! JSON-LD is the path production actually takes for every product page — all
//! five captures carry a complete `Product` block — so it gets the most
//! coverage. `parse_from_js_globals` is fed a side-fixture transcribed from a
//! captured page's inline `<script>`; see `tests/fixtures/README.md`.

use iherb_cli::scraper::product::{parse_from_js_globals, parse_from_json_ld};

use crate::fixture::{self, BASE_URL, B_COMPLEX, GOLD_C_POWDER, OLLY_GUMMIES, TWO_A_DAY};

#[test]
fn json_ld_is_present_and_complete_on_every_product_page() {
    for f in fixture::products() {
        let product = parse_from_json_ld(&f.json_ld(), f.product_id(), BASE_URL)
            .unwrap_or_else(|| panic!("{}: JSON-LD did not parse", f.slug()));

        assert!(!product.name.is_empty(), "{}", f.slug());
        assert!(!product.brand.is_empty(), "{}", f.slug());
        assert!(product.price > 0.0, "{}", f.slug());
        assert_eq!(product.currency, "USD", "{}", f.slug());
        assert!(product.rating.is_some(), "{}", f.slug());
        assert!(product.review_count.is_some(), "{}", f.slug());
        assert!(product.description.is_some(), "{}", f.slug());
        assert_eq!(product.product_id, f.product_id());

        // `sku`/`mpn` and `gtin12` are on all five, which is why #2's DOM
        // label bug has stayed invisible: JSON-LD fills these in first.
        assert!(product.product_code.is_some(), "{}", f.slug());
        assert!(product.upc.is_some(), "{}", f.slug());

        // No capture carries `url`, so every product URL is synthesised.
        assert_eq!(
            product.product_url,
            format!("{}/pr/p/{}", BASE_URL, f.product_id())
        );
    }
}

/// iHerb emits two different `offers` shapes. Both have to reach the same
/// `(price, original_price)`, and only the discounted page has an original.
#[test]
fn json_ld_reads_both_offer_shapes() {
    // `priceSpecification` array with a StrikethroughPrice entry.
    let discounted = parse_from_json_ld(&TWO_A_DAY.json_ld(), "104996", BASE_URL).unwrap();
    assert_eq!(discounted.price, 12.38);
    assert_eq!(discounted.original_price, Some(17.69));

    // Flat top-level `price`, empty `priceSpecification`.
    let flat = parse_from_json_ld(&B_COMPLEX.json_ld(), "108255", BASE_URL).unwrap();
    assert_eq!(flat.price, 20.23);
    assert_eq!(flat.original_price, None);

    let also_discounted = parse_from_json_ld(&GOLD_C_POWDER.json_ld(), "59561", BASE_URL).unwrap();
    assert_eq!(also_discounted.price, 7.79);
    assert_eq!(also_discounted.original_price, Some(9.17));
}

/// The gummies page is the only capture that is out of stock, and availability
/// is the one field the DOM fallback gets wrong where JSON-LD gets it right
/// (see `product_dom::dom_fallback_reports_the_gummies_as_in_stock`).
#[test]
fn json_ld_reads_out_of_stock() {
    let gummies = parse_from_json_ld(&OLLY_GUMMIES.json_ld(), "119174", BASE_URL).unwrap();
    assert!(!gummies.in_stock);
    assert_eq!(
        gummies.name,
        "OLLY, Goodbye Stress®, Berry Verbena, 42 Gummies"
    );
    assert_eq!(gummies.brand, "OLLY");

    let in_stock = parse_from_json_ld(&B_COMPLEX.json_ld(), "108255", BASE_URL).unwrap();
    assert!(in_stock.in_stock);
}

#[test]
fn json_ld_rejects_a_block_with_no_name() {
    let empty = serde_json::json!({ "@type": "Product", "name": "" });
    assert!(parse_from_json_ld(&empty, "1", BASE_URL).is_none());
    let nameless = serde_json::json!({ "@type": "Product" });
    assert!(parse_from_json_ld(&nameless, "1", BASE_URL).is_none());
}

/// CHARACTERIZATION, NOT DESIRED: pins #30. `parse_from_js_globals` cannot
/// parse the real page. It reads `ihrProduct.prdNm`; every captured page writes
/// `prdctNm`, so the name comes back empty and the whole strategy returns
/// `None` — the JS-globals rung of the fallback ladder is dead in production,
/// and the stock and UPC data it carries is discarded with it.
///
/// #30 flips this to `is_some()`, plus the field checks in
/// `js_globals_parse_when_the_expected_key_is_present` below.
#[test]
fn js_globals_never_match_the_real_page_shape() {
    let globals = fixture::json("js-globals-12949");
    assert_eq!(
        globals["ihrProduct"]["prdctNm"].as_str(),
        Some("Nordic Naturals, Ultimate Omega®, Great Lemon, 180 Soft Gels (640 mg per Soft Gel)"),
        "the fixture must keep the page's real key, or this test proves nothing"
    );
    assert!(globals["ihrProduct"].get("prdNm").is_none());

    assert!(
        parse_from_js_globals(&globals, "12949", BASE_URL, "USD").is_none(),
        "parse_from_js_globals unexpectedly succeeded — has the key been fixed?"
    );
}

/// The parser does work when handed the key it asks for, which is what makes
/// the test above a naming bug rather than a broken parser.
#[test]
fn js_globals_parse_when_the_expected_key_is_present() {
    let mut globals = fixture::json("js-globals-12949");
    let name = globals["ihrProduct"]["prdctNm"].clone();
    globals["ihrProduct"]["prdNm"] = name;

    let product = parse_from_js_globals(&globals, "12949", BASE_URL, "CHF").unwrap();
    assert_eq!(product.brand, "Nordic Naturals");
    assert_eq!(product.price, 64.56);
    assert_eq!(product.product_code.as_deref(), Some("NOR-03790"));
    // JS globals carry no currency, so the config fallback label is used as-is.
    assert_eq!(product.currency, "CHF");
}

/// #34 deleted the `__NEXT_DATA__` parsers. This is the guard that says so:
/// no captured page has ever carried the blob, and a live check on two freshly
/// fetched product pages and one search page on 2026-08-31 found none either.
/// If this test ever fails, iHerb has changed platform and the parsers are
/// worth writing again — against a real fixture this time, which is what git
/// history holds the deleted versions for.
#[test]
fn next_data_is_absent_from_every_captured_page() {
    for f in fixture::all() {
        assert!(
            !f.html().contains("__NEXT_DATA__"),
            "{} now contains __NEXT_DATA__ — see #34 before assuming the \
             parsers stay deleted",
            f.slug()
        );
    }
}
