use crate::model::Source;
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

/// The hooks iHerb hangs its own not-found page on, which carry no prose (#59).
///
/// These are the markers that matter. The three copy strings this function used
/// to check — and only check — matched nothing iHerb serves: the page a dead
/// product id returns is titled `The page is not found!`, so
/// [`crate::error::IherbError::ProductNotFound`] was raised from neither of its
/// call sites and exit code 23 had no live producer at all. Every dead id
/// exited 41 `parse_failed` instead, which is the one code the README says is
/// worth paging a human about.
///
/// Appending a fourth copy string would have fixed today and left the same trap
/// armed, because a marker list made of prose retires itself the next time
/// somebody edits the prose. `id="error-page-404"` and the `data-testid`
/// attributes are what the page's own template is built from: they are not
/// translated, they are not reworded, and a change to them is a change to the
/// page rather than to its wording.
///
/// Measured on both storefronts on 2026-09-02, captured as
/// `tests/fixtures/notfound-product-99999999*.html.gz`. The two captures are
/// byte-identical apart from the hostname in three links, so this list is one
/// page's structure and not two.
const NOT_FOUND_STRUCTURE_MARKERS: &[&str] = &[
    "id=\"error-page-404\"",
    "data-testid=\"error-page-title\"",
    "data-testid=\"error-page-content\"",
    "data-testid=\"error-page-return-links\"",
    "icon-page-not-found",
];

/// Not-found phrasing, compared case-insensitively against the document's own
/// `<title>` rather than against the whole document.
///
/// Against the title because that is where a not-found page says so, and
/// because the whole document is where a false positive comes from: a product
/// review or a description is free to contain the words "page not found", and
/// a title is not. Case-insensitively because iHerb itself spells it two ways
/// on one page — `The page is not found!` in the `<title>`, `The Page is Not
/// Found! | iHerb` in `og:title`.
const NOT_FOUND_TITLE_MARKERS: &[&str] = &[
    // What iHerb serves today, both storefronts.
    "the page is not found",
    // The three that were here before. They match nothing this repository has
    // captured, but they presumably matched *something* once, and dropping a
    // marker is a behaviour change nobody measured. They cost one pass over a
    // title string.
    "page not found",
    "404 not found",
];

/// Does this HTML say the thing being asked for does not exist? (#59)
///
/// Two independent families of signal, either of which is enough:
///
///  1. **Structure** — [`NOT_FOUND_STRUCTURE_MARKERS`], the template hooks.
///     Language-independent and copy-independent.
///  2. **The title** — [`NOT_FOUND_TITLE_MARKERS`], read out of the `<title>`
///     element and compared case-insensitively.
///
/// Either alone would work against the page iHerb serves today. Both are here
/// so that the *next* change to that page has to break two unrelated things
/// before this quietly starts answering `false` again.
///
/// A `true` here is what separates "stop asking about this id" from "the
/// scraper is broken and a human should look". Nothing about the fetch is being
/// judged — the page loaded, and `fetched_at` stays non-null on this path.
pub fn is_not_found_page(html: &str) -> bool {
    if NOT_FOUND_STRUCTURE_MARKERS
        .iter()
        .any(|marker| html.contains(marker))
    {
        return true;
    }

    // `<title>404</title>` used to be checked as a literal. A title that *is*
    // the bare status number carries no phrase to match, so it is asked as the
    // question it is rather than added to the phrase list, where `"404"` as a
    // substring would match any page mentioning the number.
    match document_title(html) {
        Some(title) => {
            let title = title.trim().to_lowercase();
            title == "404"
                || NOT_FOUND_TITLE_MARKERS
                    .iter()
                    .any(|marker| title.contains(marker))
        }
        None => false,
    }
}

/// The text inside the document's first `<title>` element, unescaped only as
/// far as a byte scan can manage — which is far enough, because every marker
/// compared against it is plain ASCII words.
///
/// A hand-rolled scan rather than `Html::parse_document`, because this runs on
/// every page the tool fetches and parsing a five-megabyte product page to read
/// forty bytes of its head is the wrong trade. `<title` rather than `<title>`
/// so an attribute on the tag does not hide it.
fn document_title(html: &str) -> Option<&str> {
    let open = html.find("<title")?;
    let rest = &html[open + "<title".len()..];
    let content_start = rest.find('>')? + 1;
    let rest = &rest[content_start..];
    let end = rest.find("</title>")?;
    Some(&rest[..end])
}

/// Symbols that name more than one currency iHerb prices in (#52).
///
/// `$` and only `$`, and the list is a list so that the reason is stated once
/// rather than buried in an `if`. iHerb serves at least seven storefronts that
/// price in a dollar — US, Canada, Australia, New Zealand, Singapore, Hong Kong
/// and Mexico — and the glyph is the same on all of them.
///
/// **`¥` is the obvious second candidate and is deliberately not here.** It is
/// JPY on the Japanese storefront and CNY on the Chinese one, which is the same
/// defect; #52 is about `$`, decided the answer for `$`, and extending the
/// decision to a symbol the issue did not weigh would be this file deciding
/// rather than reporting. Filed thinking, not a rule.
const AMBIGUOUS_CURRENCY_SYMBOLS: &[&str] = &["$"];

/// What a page's currency markers came to (#52).
///
/// Three answers rather than an `Option<String>`, because the `Option` could
/// not tell the two empty ones apart. "The page published no currency at all"
/// and "the page published a `$` and nothing here can say which dollar" are
/// different facts about the page, and #28's provenance model has a word for
/// each: [`Source::Absent`] and [`Source::Malformed`].
///
/// Both carry no value. That is the point — the alternative is to guess, and
/// guessing is what #5 spent two commits removing from this field and #49
/// removed from the search path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrencyRead {
    /// The page named its currency, and this is it.
    Stated(String),
    /// The page carried a currency symbol that names more than one of the
    /// currencies iHerb prices in, and no stronger marker resolved it.
    ///
    /// The symbol is carried so the log and any future decision can say which
    /// one it was.
    Ambiguous(&'static str),
    /// No currency marker anywhere on the page.
    Absent,
}

impl CurrencyRead {
    /// The code to put in the record, which only [`CurrencyRead::Stated`] has.
    pub fn value(self) -> Option<String> {
        match self {
            CurrencyRead::Stated(code) => Some(code),
            CurrencyRead::Ambiguous(_) | CurrencyRead::Absent => None,
        }
    }

    /// Where the record should say that value came from (#28).
    ///
    /// [`Source::Malformed`] for the ambiguous case, and it is not
    /// interchangeable with the `None` above: the `None` is the value, and this
    /// says where it came from. Together they mean "a signal was on the page
    /// and it could not be resolved", which is what `Malformed` is for, and
    /// what puts the field on the health report's rot list instead of its
    /// nothing-here list.
    pub fn source(&self) -> Source {
        match self {
            CurrencyRead::Stated(_) => Source::Dom,
            CurrencyRead::Ambiguous(_) => Source::Malformed,
            CurrencyRead::Absent => Source::Absent,
        }
    }
}

/// The currency the page itself declares, and how well it declares it.
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
///  3. The symbol the price text starts with. A last resort, and since #52 an
///     honest one: a symbol that names one currency is read as that currency,
///     and a symbol that names several is [`CurrencyRead::Ambiguous`] rather
///     than a guess at the most likely storefront.
pub fn detect_currency_from_html(doc: &Html) -> CurrencyRead {
    if let Some(code) = detect_currency_from_globals(doc) {
        tracing::debug!("Detected currency from window.CURRENCY_CODE: {}", code);
        return CurrencyRead::Stated(code);
    }

    if let Ok(sel) = Selector::parse("meta[itemprop='priceCurrency']") {
        if let Some(el) = doc.select(&sel).next() {
            if let Some(code) = el.value().attr("content") {
                let code = code.trim().to_uppercase();
                if !code.is_empty() {
                    tracing::debug!("Detected currency from meta tag: {}", code);
                    return CurrencyRead::Stated(code);
                }
            }
        }
    }

    if let Ok(sel) = Selector::parse("span.price bdi, div.price bdi, .product-price bdi") {
        if let Some(el) = doc.select(&sel).next() {
            let text: String = el.text().collect::<Vec<_>>().join("").trim().to_string();
            match detect_currency_from_text(&text) {
                CurrencyRead::Stated(currency) => {
                    tracing::debug!("Detected currency from price text: {}", currency);
                    return CurrencyRead::Stated(currency);
                }
                CurrencyRead::Ambiguous(symbol) => {
                    tracing::debug!(
                        "The price text starts with {}, which names more than one \
                         currency iHerb serves; reporting no currency rather than \
                         guessing (#52)",
                        symbol
                    );
                    return CurrencyRead::Ambiguous(symbol);
                }
                CurrencyRead::Absent => {}
            }
        }
    }

    CurrencyRead::Absent
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

/// The currency a price's leading symbol names, when it names one (#52).
///
/// A bare `$` used to be read as USD. It is USD on the US storefront and CAD,
/// AUD, NZD, SGD, HKD or MXN on six others iHerb serves, so on any of those a
/// price of `$24.99` was silently labelled USD — a guess wearing a fact's
/// clothes, and the same class of thing #5 removed from this field and #49
/// removed from the search path.
///
/// The prefixed dollars above it are checked first and still resolve: `CA$`,
/// `C$`, `A$` and `AU$` name one currency each, and a page that writes one has
/// said which dollar it means.
fn detect_currency_from_text(text: &str) -> CurrencyRead {
    let text = text.trim();
    if text.starts_with("CHF") {
        CurrencyRead::Stated("CHF".to_string())
    } else if text.starts_with("CA$") || text.starts_with("C$") {
        CurrencyRead::Stated("CAD".to_string())
    } else if text.starts_with("A$") || text.starts_with("AU$") {
        CurrencyRead::Stated("AUD".to_string())
    } else if text.starts_with('€') {
        CurrencyRead::Stated("EUR".to_string())
    } else if text.starts_with('£') {
        CurrencyRead::Stated("GBP".to_string())
    } else if text.starts_with('¥') {
        CurrencyRead::Stated("JPY".to_string())
    } else if text.starts_with('₩') {
        CurrencyRead::Stated("KRW".to_string())
    } else if let Some(symbol) = AMBIGUOUS_CURRENCY_SYMBOLS
        .iter()
        .find(|symbol| text.starts_with(*symbol))
    {
        CurrencyRead::Ambiguous(symbol)
    } else {
        CurrencyRead::Absent
    }
}
