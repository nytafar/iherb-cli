use scraper::{Html, Selector};
use std::path::PathBuf;
use std::time::SystemTime;
use time::OffsetDateTime;

/// Parse a price string by extracting digits, periods, and commas, then
/// determine the decimal separator based on position and context.
/// Handles both US format (1,234.56) and European format (1.234,56).
pub fn parse_price_str(s: &str) -> Option<f64> {
    // Keep only digits, periods, and commas
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();

    if cleaned.is_empty() {
        return None;
    }

    let has_dot = cleaned.contains('.');
    let has_comma = cleaned.contains(',');

    let normalized = if has_dot && has_comma {
        // Both present: the LAST one is the decimal separator
        let last_dot = cleaned.rfind('.').unwrap();
        let last_comma = cleaned.rfind(',').unwrap();
        if last_comma > last_dot {
            // Comma is decimal (European: 1.234,56)
            cleaned.replace('.', "").replacen(',', ".", 1)
        } else {
            // Dot is decimal (US: 1,234.56)
            cleaned.replace(',', "")
        }
    } else if has_comma {
        // Only commas: check if it looks like a thousands separator
        let last_comma = cleaned.rfind(',').unwrap();
        let after_comma = &cleaned[last_comma + 1..];
        if after_comma.len() == 3 && after_comma.chars().all(|c| c.is_ascii_digit()) {
            // Exactly 3 digits after last comma => thousands separator (e.g. "1,000")
            cleaned.replace(',', "")
        } else {
            // Otherwise treat comma as decimal (e.g. "23,99")
            cleaned.replacen(',', ".", 1)
        }
    } else {
        // Only dots or no separator at all: parse normally
        cleaned
    };

    normalized.parse().ok()
}

/// Extract text from a document by trying comma-separated CSS selectors.
pub fn extract_text(doc: &Html, selectors: &str) -> Option<String> {
    for sel_str in selectors.split(',') {
        if let Ok(sel) = Selector::parse(sel_str.trim()) {
            if let Some(element) = doc.select(&sel).next() {
                let text: String = element
                    .text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim()
                    .to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
    }
    None
}

/// Extract text from an element reference by trying comma-separated CSS selectors.
pub fn extract_element_text(el: &scraper::ElementRef, selectors: &str) -> Option<String> {
    for sel_str in selectors.split(',') {
        if let Ok(sel) = Selector::parse(sel_str.trim()) {
            if let Some(child) = el.select(&sel).next() {
                let text: String = child
                    .text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim()
                    .to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
    }
    None
}

/// Parse a review count out of the text that surrounds it.
///
/// A review count is *one* whole number, optionally written with thousands
/// separators: `42,328 Reviews`, `(1,234)`, `7`. Anything else is refused.
///
/// The refusal is the point (#37). This used to keep every digit it found and
/// concatenate them, so `"4.8/5 - 24,938 Reviews"` — the `title` attribute of
/// `a.stars` on a real search card — became `Some(48_524_938)`: a rating, a
/// scale and a count glued together into a number 1,950x too large, delivered
/// with no error to a caller who cannot check it. Picking one of the three
/// numbers would be a guess, and a guess is the failure this crate exists to
/// avoid, so a string carrying more than one number yields `None` and #28
/// records the field as absent rather than extracted.
pub fn parse_review_count(text: &str) -> Option<u32> {
    let mut numbers = number_runs(text);
    let only = numbers.next()?;
    if numbers.next().is_some() {
        // More than one number in the string: which one is the count is a
        // guess, so refuse rather than concatenate or pick.
        return None;
    }
    grouped_integer(only)
}

/// The maximal runs of digits-and-separators in `text`, each starting with a
/// digit. `"4.8/5 - 24,938 Reviews"` yields `"4.8"`, `"5"`, `"24,938"`.
fn number_runs(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_digit() || c == ',' || c == '.'))
        .map(|run| run.trim_matches([',', '.']))
        .filter(|run| run.chars().any(|c| c.is_ascii_digit()))
}

/// Read a whole number that may carry thousands separators, and nothing else.
///
/// Accepts `7`, `42,328` and `42.328`; rejects `4.8` and `1,234.56`, because a
/// separator followed by anything other than exactly three digits is a decimal
/// point, not a grouping mark, and a review count has no decimals.
fn grouped_integer(run: &str) -> Option<u32> {
    let mut groups = run.split([',', '.']);

    let first = groups.next()?;
    if first.is_empty() || first.len() > 3 || !first.chars().all(|c| c.is_ascii_digit()) {
        // A leading group longer than three digits is only ever an ungrouped
        // number, which has no separators at all.
        return if run.chars().all(|c| c.is_ascii_digit()) {
            run.parse().ok()
        } else {
            None
        };
    }

    let mut digits = first.to_string();
    for group in groups {
        if group.len() != 3 || !group.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        digits.push_str(group);
    }
    digits.parse().ok()
}

/// The name a dump of `label`, taken at `at` by process `pid`, is filed under.
///
/// `iherb_<label>_<UTC timestamp>_<pid>.html`. The label used to be the *whole*
/// name, so two runs against the same target overwrote each other and the one
/// thing a kept dump is for — diffing what iHerb served before and after it
/// changed something — needed files moved by hand (#63). The timestamp is
/// milliseconds and sorts lexically, which is also chronologically; the pid is
/// what keeps two processes fetching the same id in the same millisecond off
/// each other's file, which #10's batch work makes likelier rather than less.
///
/// `at` and `pid` are arguments rather than read inside, so a test can name two
/// instants and see two names.
pub fn dump_file_name(label: &str, at: SystemTime, pid: u32) -> String {
    let safe_label: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // A search label is a user's query, so anything can be in it. Replacing a
    // space was never enough: a `/` made the write land somewhere else or fail,
    // silently either way, because the write is deliberately unchecked.
    let at = OffsetDateTime::from(at);
    format!(
        "iherb_{}_{:04}{:02}{:02}T{:02}{:02}{:02}.{:03}Z_{}.html",
        safe_label,
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        at.millisecond(),
        pid,
    )
}

/// Where a dump of `label` taken right now would go.
pub fn dump_path(label: &str) -> PathBuf {
    crate::config::dumps_dir().join(dump_file_name(label, SystemTime::now(), std::process::id()))
}

/// Dump HTML under the cache directory for debugging when debug level is
/// enabled.
///
/// The write stays unchecked: a full disk or an unwritable cache directory must
/// cost a diagnostic, never the run that was asked for.
pub fn debug_dump_html(html: &str, label: &str) {
    if tracing::enabled!(tracing::Level::DEBUG) {
        let path = dump_path(label);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, html);
        tracing::debug!("Dumped HTML to {}", path.display());
    }
}

/// Check if HTML indicates a 404/not-found page.
pub fn is_not_found_page(html: &str) -> bool {
    html.contains("Page Not Found")
        || html.contains("<title>404</title>")
        || html.contains("404 Not Found")
}

/// The currency the page itself declares, or `None` when it declares none.
///
/// Three readings, strongest first.
///
///  1. `window.CURRENCY_CODE`, which iHerb's own header script writes into every
///     page and every page of the seven captures carries. It is the storefront's
///     answer for the whole document rather than for one price, and it is the
///     value the site's own GraphQL calls are built from
///     (`?country=…&currency=`+`CURRENCY_CODE`). It goes first because it is the
///     only one of the three that is unambiguous.
///  2. `<meta itemprop="priceCurrency">`, the microdata on a product page.
///  3. The symbol the price text starts with — a guess, and known to be a weak
///     one: a bare `$` is USD on the US storefront and CAD, AUD, SGD, HKD, NZD
///     or MXN on six others iHerb serves. It is kept as a last resort because
///     it is still a reading of the page, but the rung above it exists so that
///     it is almost never reached.
pub fn detect_currency_from_html(doc: &Html) -> Option<String> {
    if let Some(code) = detect_currency_from_globals(doc) {
        tracing::debug!("Detected currency from window.CURRENCY_CODE: {}", code);
        return Some(code);
    }

    if let Ok(sel) = Selector::parse("meta[itemprop='priceCurrency']") {
        if let Some(el) = doc.select(&sel).next() {
            if let Some(code) = el.value().attr("content") {
                let code = code.trim().to_uppercase();
                if !code.is_empty() {
                    tracing::debug!("Detected currency from meta tag: {}", code);
                    return Some(code);
                }
            }
        }
    }

    if let Ok(sel) = Selector::parse("span.price bdi, div.price bdi, .product-price bdi") {
        if let Some(el) = doc.select(&sel).next() {
            let text: String = el.text().collect::<Vec<_>>().join("").trim().to_string();
            if let Some(currency) = detect_currency_from_text(&text) {
                tracing::debug!("Detected currency from price text: {}", currency);
                return Some(currency);
            }
        }
    }

    None
}

/// Read `window.CURRENCY_CODE = "XXX"` out of the page's inline scripts.
///
/// Anchored on the assignment rather than on the name alone. The name occurs
/// three other ways in the same bundle — `window.CURRENCY_CODE` read back in a
/// URL concatenation, and `CURRENCY_CODE:"currencyCode"` in a map of
/// feature-flag property names — and only the assignment carries the
/// storefront's code. Requiring `=` and then exactly three A-Z letters in
/// quotes is what tells them apart; the property-name entry uses `:` and holds
/// a lowerCamelCase word, so it can match neither test.
pub fn detect_currency_from_globals(doc: &Html) -> Option<String> {
    let sel = Selector::parse("script").ok()?;
    doc.select(&sel)
        .find_map(|el| currency_assignment(&el.text().collect::<String>()))
}

/// The first `CURRENCY_CODE = "XXX"` in one script's source.
fn currency_assignment(script: &str) -> Option<String> {
    let mut rest = script;
    while let Some(idx) = rest.find("CURRENCY_CODE") {
        let after = rest[idx + "CURRENCY_CODE".len()..].trim_start();
        rest = &rest[idx + "CURRENCY_CODE".len()..];

        let Some(after) = after.strip_prefix('=') else {
            continue;
        };
        let after = after.trim_start();
        let Some(after) = after.strip_prefix(['"', '\'']) else {
            continue;
        };
        let code: String = after.chars().take(3).collect();
        // Exactly three A-Z letters, and the quote has to close right after
        // them: `"USDX"` is not a currency code and must not be read as `USD`.
        if code.len() == 3
            && code.chars().all(|c| c.is_ascii_uppercase())
            && after[code.len()..].starts_with(['"', '\''])
        {
            return Some(code);
        }
    }
    None
}

fn detect_currency_from_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.starts_with('$') {
        Some("USD".to_string())
    } else if text.starts_with('€') {
        Some("EUR".to_string())
    } else if text.starts_with('£') {
        Some("GBP".to_string())
    } else if text.starts_with("CHF") {
        Some("CHF".to_string())
    } else if text.starts_with("CA$") || text.starts_with("C$") {
        Some("CAD".to_string())
    } else if text.starts_with("A$") || text.starts_with("AU$") {
        Some("AUD".to_string())
    } else if text.starts_with("¥") {
        Some("JPY".to_string())
    } else if text.starts_with("₩") {
        Some("KRW".to_string())
    } else {
        None
    }
}
