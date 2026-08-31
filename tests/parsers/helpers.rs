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

/// FLIPPED BY #37. This asserted the old behaviour: `parse_review_count` kept
/// every digit it found, wherever it found it, so `"4.8/5 - 24,938 Reviews"`
/// — the `title` attribute of `a.stars` on the captured search page, which
/// `extract_card_rating` already reads — was concatenated into
/// `Some(48_524_938)`, reporting 24,938 reviews as 48.5 million.
///
/// #37 offered `Some(24_938)` or `None`. `None` is what landed: choosing the
/// count out of three numbers is a guess, and this crate exists not to guess.
/// A caller gets no review count and #28 records the field as absent, which is
/// a thing an agent can act on; a number 1,950x too large is not.
#[test]
fn a_string_carrying_more_than_one_number_has_no_review_count() {
    assert_eq!(parse_review_count("4.8/5 - 24,938 Reviews"), None);

    // The string really is on the page, so this is one selector change away
    // from being live.
    assert!(SEARCH_VITAMIN_C
        .html()
        .contains(r#"title="4.8/5 - 24,938 Reviews""#));
}

/// The boundary the refusal is drawn on: one whole number, thousands
/// separators allowed, is a count; anything with a fractional part, and
/// anything with a second number in it, is not.
#[test]
fn only_a_lone_whole_number_is_a_review_count() {
    // Grouped either way round, because a European storefront groups with dots.
    assert_eq!(parse_review_count("42.328 Reviews"), Some(42_328));
    assert_eq!(parse_review_count("1.234.567"), Some(1_234_567));

    // A lone rating is not a count: the old parser answered `Some(48)`.
    assert_eq!(parse_review_count("4.8"), None);
    assert_eq!(parse_review_count("4.8 out of 5"), None);

    // Nor is a price: the old parser answered `Some(123456)`.
    assert_eq!(parse_review_count("$1,234.56"), None);

    // Two counts is not one count, however they are written.
    assert_eq!(parse_review_count("12 of 3,456 Reviews"), None);

    // Beyond u32 there is no count to report, rather than a wrapped one.
    assert_eq!(parse_review_count("99,999,999,999"), None);
}

#[test]
fn currency_is_detected_from_the_captured_pages() {
    for f in fixture::products() {
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
