//! The concrete things [`crate::fetch::fetch`] knows how to fetch.
//!
//! Each target is a descriptor: cache identity, URLs, extraction, validation.
//! A new command adds a module here; it does not repeat the pipeline.

pub mod product;
pub mod search;

pub use product::ProductTarget;
pub use search::SearchTarget;

use crate::error::IherbError;

/// Check a fetched record's currency against what `--currency` asked for (#5).
///
/// `--currency` is a requirement, not a label. It cannot convert — iHerb prices
/// in the currency of the storefront `--country` selects — so the only thing it
/// can do about a storefront that prices in something else is refuse to answer.
/// The alternative is what shipped before: the flag was a fallback label, so
/// `--currency CHF` against the US storefront produced US prices captioned CHF
/// whenever currency detection happened to fail, and produced nothing at all
/// when it happened to work.
///
/// `expected` of `None` is the default and asserts nothing: whatever the
/// storefront prices in is what you get.
///
/// An `actual` of `None` fails too. A page that published no currency cannot
/// confirm the one that was asked for, and "we could not tell" is not
/// confirmation — the same distinction `in_stock` draws (#31).
pub fn check_currency(
    expected: Option<&str>,
    actual: Option<&str>,
    what: &str,
) -> Result<(), IherbError> {
    let Some(expected) = expected else {
        return Ok(());
    };

    match actual {
        Some(actual) if actual.eq_ignore_ascii_case(expected) => Ok(()),
        Some(actual) => Err(IherbError::CurrencyMismatch {
            expected: expected.to_string(),
            actual: actual.to_string(),
            what: what.to_string(),
        }),
        None => Err(IherbError::CurrencyMismatch {
            expected: expected.to_string(),
            actual: "unknown — the page published none".to_string(),
            what: what.to_string(),
        }),
    }
}
