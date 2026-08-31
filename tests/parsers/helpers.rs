//! `parse_price_str`, `parse_review_count`, `detect_currency_from_html`.

use iherb_cli::scraper::helpers::{detect_currency_from_html, parse_price_str, parse_review_count};
use scraper::Html;

use crate::fixture::{self, B_COMPLEX, SEARCH_VITAMIN_C};

#[test]
fn prices_parse_in_both_decimal_conventions() {
    // US: dot is the decimal separator, comma groups thousands.
    assert_eq!(parse_price_str("$9.60"), Some(9.60));
    assert_eq!(parse_price_str("$1,234.56"), Some(1234.56));
    assert_eq!(parse_price_str("1,000"), Some(1000.0));

    // European: comma is the decimal separator.
    assert_eq!(parse_price_str("€1.234,56"), Some(1234.56));
    assert_eq!(parse_price_str("23,99"), Some(23.99));

    // Currency prefixes and suffixes are stripped, whatever they are.
    assert_eq!(parse_price_str("CHF 12.00"), Some(12.00));
    assert_eq!(parse_price_str("12.00 kr"), Some(12.00));
    assert_eq!(parse_price_str(" 7.79 "), Some(7.79));
}

#[test]
fn a_string_with_no_digits_is_not_a_price() {
    assert_eq!(parse_price_str(""), None);
    assert_eq!(parse_price_str("Free"), None);
    assert_eq!(parse_price_str("$"), None);
}

#[test]
fn review_counts_survive_their_surrounding_text() {
    assert_eq!(parse_review_count("42,328 Reviews"), Some(42_328));
    assert_eq!(parse_review_count("(1,234)"), Some(1_234));
    assert_eq!(parse_review_count("7"), Some(7));
    assert_eq!(parse_review_count("Reviews"), None);
    assert_eq!(parse_review_count(""), None);
}

/// CHARACTERIZATION: `parse_review_count` keeps every digit it finds, wherever
/// it finds it, so a string carrying a second number silently concatenates.
/// Nothing on the captured pages hits this, but it is worth pinning before
/// someone widens the selector that feeds it.
#[test]
fn review_counts_concatenate_digits_from_anywhere_in_the_string() {
    assert_eq!(
        parse_review_count("4.8 out of 5, 331 reviews"),
        Some(485331)
    );
}

#[test]
fn currency_is_detected_from_the_captured_pages() {
    for &f in fixture::PRODUCTS {
        assert_eq!(
            detect_currency_from_html(&f.doc()).as_deref(),
            Some("USD"),
            "{}",
            f.slug()
        );
    }
    assert_eq!(
        detect_currency_from_html(&SEARCH_VITAMIN_C.doc()).as_deref(),
        Some("USD")
    );
}

#[test]
fn currency_comes_from_the_meta_tag_first_and_the_price_text_second() {
    let meta = Html::parse_document(r#"<meta itemprop="priceCurrency" content="chf">"#);
    assert_eq!(detect_currency_from_html(&meta).as_deref(), Some("CHF"));

    for (text, expected) in [
        ("$9.60", "USD"),
        ("€9,60", "EUR"),
        ("£9.60", "GBP"),
        ("CHF 9.60", "CHF"),
        ("CA$9.60", "CAD"),
        ("A$9.60", "AUD"),
        ("¥960", "JPY"),
        ("₩9600", "KRW"),
    ] {
        let doc = Html::parse_document(&format!(
            r#"<span class="price"><bdi>{}</bdi></span>"#,
            text
        ));
        assert_eq!(
            detect_currency_from_html(&doc).as_deref(),
            Some(expected),
            "{}",
            text
        );
    }
}

/// The case #5 is actually about: when the page carries no currency marker the
/// caller substitutes its own `--currency` label, and a US price is then
/// printed as CHF. This is where detection has to fail for that to happen.
#[test]
fn currency_detection_falls_through_on_a_page_with_no_markers() {
    assert_eq!(detect_currency_from_html(&fixture::empty_doc()), None);

    // An unrecognised symbol is a fall-through too, not a guess.
    let doc = Html::parse_document(r#"<span class="price"><bdi>R$ 9,60</bdi></span>"#);
    assert_eq!(detect_currency_from_html(&doc), None);

    // The captured pages never take that branch — B_COMPLEX has the meta tag.
    assert!(B_COMPLEX.html().contains("priceCurrency"));
}
