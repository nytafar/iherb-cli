use crate::cli::Section;
use crate::config::AppConfig;
use crate::error::ErrorKind;
use crate::model::{ExtractionHealth, ProductDetail, SearchResult, Source};
use serde::Serialize;
use serde_json::{Map, Value};
use std::time::SystemTime;

pub fn format_search_results(result: &SearchResult) -> String {
    let mut out = String::new();

    let total_str = match result.total_results {
        Some(total) => format!("{}+", format_number(total)),
        None => "?".to_string(),
    };
    let showing = result.products.len();
    out.push_str(&format!(
        "## Search results for \"{}\" (showing {} of {})\n\n",
        result.query, showing, total_str
    ));

    for (i, product) in result.products.iter().enumerate() {
        out.push_str(&format!("### {}. {}\n", i + 1, product.name));
        out.push_str(&format!("- **Brand:** {}\n", product.brand));

        // A card with no price prints one, honestly. It used to print `$0.00`,
        // which reads as free (#49).
        let price_str = match product.price {
            Some(price) => format_price(
                price,
                product.original_price.as_ref(),
                product.currency.as_deref(),
            ),
            None => "Unknown (no price could be read from the card)".to_string(),
        };
        out.push_str(&format!("- **Price:** {}\n", price_str));

        if let (Some(rating), Some(count)) = (product.rating, product.review_count) {
            out.push_str(&format!(
                "- **Rating:** {:.1}/5 ({} reviews)\n",
                rating,
                format_number(count)
            ));
        }

        out.push_str(&format!("- **ID:** {}\n", product.product_id));
        out.push_str(&format!("- **URL:** {}\n", product.product_url));

        if i < result.products.len() - 1 {
            out.push_str("\n---\n\n");
        }
    }

    out
}

/// One line when a search returned fewer distinct products than `--limit`
/// asked for, saying which kind of shortfall it is. `None` when there is none.
///
/// `--limit` counts distinct products, not cards: iHerb places some products
/// twice on a results page and #33 returns each of them once. So a short result
/// is normal and needs explaining rather than hiding — and the two reasons for
/// it call for opposite responses. If iHerb ran out, there is nothing to do. If
/// the walk stopped at its page budget, there are more products behind this
/// one and asking again with a larger `--limit` reaches them.
///
/// A record that says nothing about its walk — an entry written before #6 — is
/// reported as unknown rather than as either.
pub fn format_search_shortfall(result: &SearchResult, limit: usize) -> Option<String> {
    if result.products.len() >= limit {
        return None;
    }
    let short = format!(
        "- **Fewer than --limit:** asked for {}, returning {} distinct products",
        limit,
        result.products.len()
    );
    Some(match result.fetch.exhausted {
        Some(true) => format!("{} — iHerb had no more to give.\n", short),
        Some(false) => format!(
            "{} — the walk stopped at its page budget, not at the end of the \
             results, so there are more behind these.\n",
            short
        ),
        None => format!(
            "{}. This record does not say whether the results ran out or the \
             walk did.\n",
            short
        ),
    })
}

/// What a `product` invocation shows, decided once from `--section`.
///
/// The flag used to be handed to the renderer, which then decided the layout
/// from it — `format_product_detail` branched on `section.is_some()` to work
/// out whether supplement facts belonged under Ingredients. That was fine while
/// there was one renderer. `--json` is a second one (#9), and two renderers
/// each re-deriving the layout from the same flag are two renderers that will
/// drift: the day someone changes what `--section ingredients` means, they
/// change it in one of the two.
///
/// So the decision is made here, once, and both renderings consume the answer.
/// Markdown walks [`ProductView::sections`] and prints each; `--json` walks the
/// same list and keeps each section's fields. Neither one asks what flag was
/// passed, and neither can disagree with the other about it.
#[derive(Debug, Clone)]
pub struct ProductView {
    sections: Vec<Section>,
    titled: bool,
    requested: Option<Section>,
}

impl ProductView {
    /// Resolve `--section` into what actually gets shown.
    pub fn for_section(section: Option<Section>) -> Self {
        match section {
            // The whole record, under the product's name.
            None => Self {
                sections: Section::ALL.to_vec(),
                titled: true,
                requested: None,
            },

            // Supplement facts *are* the active ingredients, and a supplement
            // label reads them above the inactive ones. So asking for
            // ingredients on their own asks for both, and that is a fact about
            // the request rather than about how it is drawn: it belongs in the
            // resolved section list, where every rendering sees it, and not in
            // an `if` inside one of them.
            //
            // It does not apply to the whole record, where Nutrition already
            // has its own place in the running order.
            Some(Section::Ingredients) => Self {
                sections: vec![Section::Nutrition, Section::Ingredients],
                titled: false,
                requested: Some(Section::Ingredients),
            },

            Some(s) => Self {
                sections: vec![s],
                titled: false,
                requested: Some(s),
            },
        }
    }

    /// The whole record. What an invocation with no `--section` shows.
    pub fn everything() -> Self {
        Self::for_section(None)
    }

    /// The sections to show, in order.
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// The section the caller named, if they named one. Only for saying which
    /// one had no data.
    pub fn requested(&self) -> Option<Section> {
        self.requested
    }
}

pub fn format_product_detail(product: &ProductDetail, view: &ProductView) -> String {
    let mut out = String::new();

    if view.titled {
        out.push_str(&format!("# {}\n\n", product.name));
    }

    for sec in view.sections() {
        match sec {
            Section::Overview => format_overview(product, &mut out),
            Section::Description => format_description(product, &mut out),
            Section::Nutrition => format_nutrition(product, &mut out),
            Section::Ingredients => format_ingredients(product, &mut out),
            Section::SuggestedUse => format_suggested_use(product, &mut out),
            Section::Warnings => format_warnings(product, &mut out),
            Section::Reviews => format_reviews(product, &mut out),
        }
    }

    if out.is_empty() {
        if let Some(sec) = view.requested() {
            out.push_str(&format!(
                "No {} data available for this product.\n",
                sec.label()
            ));
        }
    }

    out
}

fn format_overview(product: &ProductDetail, out: &mut String) {
    out.push_str("## Overview\n");
    out.push_str(&format!("- **Brand:** {}\n", product.brand));

    let price_str = format_price(
        product.price,
        product.original_price.as_ref(),
        product.currency.as_deref(),
    );
    out.push_str(&format!("- **Price:** {}\n", price_str));

    if let (Some(rating), Some(count)) = (product.rating, product.review_count) {
        out.push_str(&format!(
            "- **Rating:** {:.1}/5 ({} reviews)\n",
            rating,
            format_number(count)
        ));
    }

    // `None` is not "no": no signal on the page said either way, and printing
    // "In Stock" for that is the fabrication #31 was filed about.
    let stock_str = match product.in_stock {
        Some(true) => "In Stock",
        Some(false) => "Out of Stock",
        None => "Unknown (no availability signal found on the page)",
    };
    out.push_str(&format!("- **Availability:** {}\n", stock_str));

    if let Some(ref code) = product.product_code {
        out.push_str(&format!("- **Product Code:** {}\n", code));
    }
    if let Some(ref weight) = product.shipping_weight {
        out.push_str(&format!("- **Shipping Weight:** {}\n", weight));
    }

    // One line, only when something a product page always publishes is missing.
    // Silence is the normal case; a caller that wants the whole picture calls
    // `format_extraction_health`.
    let health = product.health();
    if health.degraded {
        out.push_str(&format!(
            "- **Data quality:** degraded — {}. Run with --debug for the full provenance table.\n",
            degradation_reason(&health)
        ));
    }

    out.push('\n');
}

/// Why this record is degraded, as a clause the caller can read.
///
/// Degradation has two causes and they need different words. A field nobody
/// produced is "no strategy produced X"; a field the page carried and we could
/// not read is "X was on the page and could not be read" — the caller's next
/// move differs, and so does whose fault it is.
///
/// The malformed clause is not optional garnish. `degraded` fires on any
/// malformed field, expected or not, and `review_distribution` — the live case
/// (#32) — is deliberately not in `EXPECTED_FIELDS`. Naming only the expected
/// fields printed an empty list and a dangling full stop for exactly that
/// record.
fn degradation_reason(health: &ExtractionHealth) -> String {
    let mut clauses = Vec::new();

    let unread = unread_expected_fields(health);
    if !unread.is_empty() {
        clauses.push(format!("no strategy produced {}", unread.join(", ")));
    }
    if !health.fields_malformed.is_empty() {
        clauses.push(format!(
            "{} {} on the page and could not be read",
            health.fields_malformed.join(", "),
            if health.fields_malformed.len() == 1 {
                "was"
            } else {
                "were"
            }
        ));
    }

    // `degraded` is true and neither list is populated: unreachable through
    // `health()`, which derives all three from the same sources, but a caller
    // can hand-build an `ExtractionHealth`. Say something true rather than
    // trail off mid-sentence, which is what this function exists to stop.
    if clauses.is_empty() {
        return "extraction reported itself unhealthy".to_string();
    }
    clauses.join("; and ")
}

/// The expected fields no strategy read, absent and defaulted together.
///
/// `degraded` is true when any expected field was not read, and a field can go
/// unread in two ways: nothing produced it, or something substituted a
/// constant. Reporting only the absent ones would print an empty list for a
/// record degraded purely by a defaulted currency.
fn unread_expected_fields(health: &ExtractionHealth) -> Vec<String> {
    ProductDetail::EXPECTED_FIELDS
        .iter()
        .filter(|f| {
            health
                .sources
                .get(**f)
                .is_some_and(|source| !source.is_attested())
        })
        .map(|f| (*f).to_string())
        .collect()
}

fn format_description(product: &ProductDetail, out: &mut String) {
    if let Some(ref desc) = product.description {
        out.push_str("## Description\n");
        out.push_str(desc);
        out.push('\n');

        // A description that came from the page HTML rather than its structured
        // data is the `<meta name="description">` fallback, which iHerb writes
        // as a ~160-character summary. It stops mid-phrase — page 104996's ends
        // "…California Gold Nutrition® Multivitamin and" — and printing that
        // unmarked shows a reader a sentence that simply stops, as if the
        // product's description really were that. Same honesty rule as the rest
        // of this wave: do not present a lesser value as the real one.
        //
        // When #13 lands and reads the full `#product-overview` markup, that
        // will be `Source::Dom` too and this test stops distinguishing them.
        // #13 owns splitting the source finely enough to tell them apart.
        if product.source_of("description") == Source::Dom {
            out.push_str(
                "\n*Read from the page's `<meta name=\"description\">`, not its structured \
                 data — iHerb writes that as a ~160-character summary, so the text above \
                 may stop mid-sentence. The full description is in the page overview, \
                 which the parser does not read yet (#13).*\n",
            );
        }

        out.push('\n');
    }
}

fn format_nutrition(product: &ProductDetail, out: &mut String) {
    let facts = match product.supplement_facts {
        Some(ref f) => f,
        None => return,
    };
    out.push_str("## Supplement Facts\n");
    if !facts.nutrients.is_empty() {
        out.push_str("| Nutrient | Amount | % Daily Value |\n");
        out.push_str("|---|---|---|\n");
        for nutrient in &facts.nutrients {
            let dv = nutrient.daily_value.as_deref().unwrap_or("");
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                nutrient.name, nutrient.amount, dv
            ));
        }
        out.push('\n');
    }
    if let Some(ref size) = facts.serving_size {
        out.push_str(&format!("- **Serving Size:** {}\n", size));
    }
    if let Some(ref servings) = facts.servings_per_container {
        out.push_str(&format!("- **Servings Per Container:** {}\n", servings));
    }
    out.push('\n');
}

fn format_ingredients(product: &ProductDetail, out: &mut String) {
    if let Some(ref ingredients) = product.ingredients {
        out.push_str("## Other Ingredients\n");
        out.push_str(ingredients);
        out.push_str("\n\n");
    }
}

fn format_suggested_use(product: &ProductDetail, out: &mut String) {
    if let Some(ref usage) = product.suggested_use {
        out.push_str("## Suggested Use\n");
        out.push_str(usage);
        out.push_str("\n\n");
    }
}

fn format_warnings(product: &ProductDetail, out: &mut String) {
    if let Some(ref warnings) = product.warnings {
        out.push_str("## Warnings\n");
        out.push_str(warnings);
        out.push_str("\n\n");
    }
}

fn format_reviews(product: &ProductDetail, out: &mut String) {
    let dist = match product.review_distribution {
        Some(ref d) => d,
        None => return,
    };
    out.push_str("## Reviews\n");
    if let (Some(rating), Some(count)) = (product.rating, product.review_count) {
        out.push_str(&format!("- **Average:** {:.1}/5\n", rating));
        out.push_str(&format!("- **Total:** {} reviews\n", format_number(count)));
    }
    if let Some(pct) = dist.five_star {
        out.push_str(&format!("- 5 stars: {:.0}%\n", pct));
    }
    if let Some(pct) = dist.four_star {
        out.push_str(&format!("- 4 stars: {:.0}%\n", pct));
    }
    if let Some(pct) = dist.three_star {
        out.push_str(&format!("- 3 stars: {:.0}%\n", pct));
    }
    if let Some(pct) = dist.two_star {
        out.push_str(&format!("- 2 stars: {:.0}%\n", pct));
    }
    if let Some(pct) = dist.one_star {
        out.push_str(&format!("- 1 star: {:.0}%\n", pct));
    }
    out.push('\n');
}

/// A price, with the currency the page published — or with a number and an
/// explicit statement that nothing named its currency (#5).
///
/// `None` prints the bare number and says so. It must never print as though the
/// currency were one we know: an unlabelled `12.38` a reader can see is
/// unlabelled costs them a second query, and `CHF 12.38` over a US price costs
/// them the wrong decision.
fn format_price(price: f64, original: Option<&f64>, currency: Option<&str>) -> String {
    let symbol = match currency {
        Some("USD") => "$",
        Some("CHF") => "CHF ",
        Some("EUR") => "€",
        Some("GBP") => "£",
        Some(other) => other,
        None => "",
    };
    let unnamed = match currency {
        Some(_) => "",
        None => " (currency unknown: the page published none)",
    };

    match original {
        Some(orig) if *orig > price => {
            let discount = ((*orig - price) / *orig * 100.0).round() as u32;
            format!(
                "{}{:.2} ~~{}{:.2}~~ ({}% off){}",
                symbol, price, symbol, orig, discount, unnamed
            )
        }
        _ => format!("{}{:.2}{}", symbol, price, unnamed),
    }
}

pub fn format_cached_at(cached_at: SystemTime) -> String {
    let t = Utc::from(cached_at);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC",
        t.year, t.month, t.day, t.hour, t.minute
    )
}

/// The same instant as RFC 3339 in UTC, e.g. `2026-08-31T09:14:22Z`.
///
/// What `--json`'s envelope carries (#44). The human line above rounds to the
/// minute, which is the right resolution to read and the wrong one to store: a
/// consumer comparing two records needs the seconds, and needs a format it can
/// parse rather than one it has to recognise.
pub fn format_rfc3339(at: SystemTime) -> String {
    let t = Utc::from(at);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// A civil date and time in UTC, broken out of a [`SystemTime`].
///
/// Hand-rolled rather than pulled from a date crate, which is the choice this
/// file already made; it is factored out here only because there are now two
/// renderings of one instant and they must not disagree about which day it is.
struct Utc {
    year: i64,
    month: usize,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
}

impl From<SystemTime> for Utc {
    fn from(at: SystemTime) -> Self {
        let secs = at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let days = secs / 86400;
        let remaining = secs % 86400;
        let hour = remaining / 3600;
        let minute = (remaining % 3600) / 60;
        let second = remaining % 60;

        // Calculate year/month/day from epoch days
        let mut y = 1970i64;
        let mut d = days;
        loop {
            let days_in_year = if is_leap(y) { 366 } else { 365 };
            if d < days_in_year {
                break;
            }
            d -= days_in_year;
            y += 1;
        }
        let month_days = [
            31,
            if is_leap(y) { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        let mut m = 0usize;
        for (i, &md) in month_days.iter().enumerate() {
            if d < md {
                m = i;
                break;
            }
            d -= md;
        }

        Utc {
            year: y,
            month: m + 1,
            day: d + 1,
            hour,
            minute,
            second,
        }
    }
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn format_number(n: u32) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

/// Render a scrape's report on itself as plain text.
///
/// **This is the seam #9 renders under `--json`.** `--json` does not exist yet
/// — it is #9, and #9 depends on this issue — so provenance gets a plain-text
/// surface here and a machine-readable one there. What #9 has to emit is
/// [`ExtractionHealth`] verbatim: it already derives `Serialize`, the field
/// names are the JSON keys, and `Source`/`Strategy` serialize as snake_case
/// strings (`json_ld`, `js_globals`, `dom`, `absent`, `unrecorded`). So:
///
/// ```json
/// "extraction": {
///   "strategy": "json_ld",
///   "enriched": true,
///   "sources": { "name": "json_ld", "ingredients": "dom", "warnings": "absent",
///                "product_url": "defaulted", ... },
///   "fields_absent": ["warnings", "..."],
///   "fields_defaulted": ["product_url", "..."],
///   "fields_malformed": ["review_distribution", "..."],
///   "degraded": false
/// }
/// ```
///
/// Two of the source values need a word of explanation to a consumer.
///
/// `defaulted`: the field has a value and nobody read it off the page — a
/// hardcoded constant, or a label passed on the command line. It is not
/// `absent`, because absent means there is nothing there; it is the more
/// dangerous of the two, because a defaulted value looks exactly like data.
///
/// `malformed`: the page carried the field and extraction could not read it.
/// There is no value, and unlike `absent` that is our fault rather than the
/// page's. Any field in `fields_malformed` sets `degraded`, whether or not it
/// is one of the fields every product page publishes — `review_distribution` is
/// the live case (#32) and is deliberately not on that list.
///
/// `serde_json::to_value(product.health())` produces exactly that. #9 needs to
/// add no fields and compute nothing; it needs to place the block and map
/// `degraded` onto its exit-code taxonomy.
pub fn format_extraction_health(health: &ExtractionHealth) -> String {
    let mut out = String::new();
    out.push_str("## Extraction\n");
    out.push_str(&format!("- **Strategy:** {:?}\n", health.strategy));
    out.push_str(&format!("- **Enriched from DOM:** {}\n", health.enriched));
    out.push_str(&format!(
        "- **Degraded:** {}\n",
        if health.degraded {
            "yes — a field every product page publishes was not read off it, or \
             a field the page carried could not be read, so the selectors may \
             have rotted"
        } else {
            "no"
        }
    ));

    if !health.fields_absent.is_empty() {
        out.push_str(&format!(
            "- **Absent:** {}\n",
            health.fields_absent.join(", ")
        ));
    }
    if !health.fields_defaulted.is_empty() {
        out.push_str(&format!(
            "- **Defaulted (a value nobody read):** {}\n",
            health.fields_defaulted.join(", ")
        ));
    }
    if !health.fields_malformed.is_empty() {
        out.push_str(&format!(
            "- **Malformed (on the page, unreadable):** {}\n",
            health.fields_malformed.join(", ")
        ));
    }

    out.push_str("\n| Field | Source |\n|---|---|\n");
    for (field, source) in &health.sources {
        out.push_str(&format!("| {} | {:?} |\n", field, source));
    }
    out.push('\n');

    out
}

// ---------------------------------------------------------------------------
// `--json`: the versioned envelope (#44) and the two payloads inside it (#9).
// ---------------------------------------------------------------------------

/// The version of the `data` shape inside the envelope (#44).
///
/// Bumped **only** on a breaking change to `data`: a field removed, a field
/// re-typed, or a field whose meaning changes. Adding a field is not breaking
/// and does not bump it — a consumer that ignores unknown keys keeps working,
/// and a version that moves for every addition tells nobody anything.
///
/// The README carries the history and the policy. Adding to it is part of
/// making the change, not a follow-up: a stored record's only claim about its
/// own shape is this integer, and there is no way to add one retroactively to
/// records already on disk.
///
/// # Version 1 has already had a breaking change, and it is still 1
///
/// `ff35741` renamed `meta.country`, `meta.currency` and `meta.storefront` to
/// `requested_country`, `requested_currency` and `requested_storefront`. Three
/// fields removed and three added under a version that did not move is, read as
/// a rule, exactly the drift this constant exists to prevent: a consumer
/// holding a document from the parent commit and one from this one cannot tell
/// them apart, because the only discriminator says `1` on both.
///
/// It is correct anyway, and only because of a fact outside the code: **nothing
/// has been released**. The unprefixed names existed on `origin` in `0b65f46`
/// for roughly twenty minutes, there is no tagged version and no published
/// artefact, so no document with a `meta.country` in it can be in anyone's
/// hands. Bumping to 2 would spend the tool's first version increment
/// announcing a break from a shape nobody ever received, and would leave a `1`
/// in the history table describing a document that does not exist.
///
/// **That assumption expires at the first release.** From the first tag
/// onwards, any change of this kind bumps this constant and adds a row to the
/// README's table — including a rename, which is a field removed and a field
/// added however sympathetic the motive.
pub const SCHEMA_VERSION: u32 = 1;

pub use crate::fetch::Provenance;

/// The invocation a JSON document came out of, carried with it.
///
/// Once the document leaves the process the invocation is gone, and a price
/// with no storefront, currency or timestamp attached is not interpretable —
/// only plausible (#44).
///
/// # Why every storefront field is named `requested_`
///
/// Because that is what they are, and the unprefixed names claimed otherwise.
/// `meta` sits beside `data` in the envelope and reads as a description of it,
/// and three of its fields described the *request* instead: with no flags on a
/// Norwegian IP the configuration resolves country `us`, so the envelope said
/// `country: "us"`, `storefront: "https://www.iherb.com"` and
/// `currency: null` — while the record beside it carried a price in NOK. #5
/// measured exactly that: iHerb geolocates by IP and overrides an unstated
/// `--country`. A consumer reading `meta.currency` as the currency of the price
/// got `null` for a NOK price, and one reading `meta.storefront` got a claim no
/// part of the run had checked.
///
/// So `meta` now says only what the run asked for, in field names that cannot
/// be read as anything else. **What the storefront actually answered is on the
/// record**, where it was measured and where its provenance is:
/// `data.currency` with `data.extraction.sources.currency`, and
/// `data.product_url` with `data.extraction.sources.product_url`. It is not
/// copied here — a value stored twice is a value that can disagree with itself,
/// and the disagreement is what this whole issue is about.
///
/// Three fields are `Option` and each `null` means something specific rather
/// than "missing":
///
///  - `fetched_at` / `from_cache` are `null` when no page was read at all —
///    every failure that happens before or instead of a fetch. A failure that
///    happens *after* a page was read reports the page, because it read one.
///  - `requested_country` / `requested_currency` / `requested_storefront` are
///    `null` when the failure happened before the configuration was resolved —
///    an unparseable command line has no effective storefront, and inventing
///    one would be a claim about a run that never started.
///  - `requested_currency` is also `null` on a perfectly good run that did not
///    pass `--currency`, because then the run asked the storefront for nothing
///    in particular.
#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    pub tool_version: &'static str,
    pub fetched_at: Option<String>,
    pub emitted_at: String,
    pub from_cache: Option<bool>,
    pub requested_country: Option<String>,
    pub requested_currency: Option<String>,
    pub requested_storefront: Option<String>,
}

impl Meta {
    /// The meta block for a run whose configuration resolved.
    ///
    /// `emitted_at` is passed in rather than read from the clock here so that
    /// what this renders is a function of its arguments — a test can hand it
    /// two instants and assert on the two strings, which is the only way the
    /// cached-versus-fresh distinction is checkable at all.
    pub fn new(config: &AppConfig, provenance: Option<Provenance>, emitted_at: SystemTime) -> Self {
        Self {
            requested_country: Some(config.country.clone()),
            requested_currency: config.currency.clone(),
            requested_storefront: Some(config.base_url()),
            ..Self::unconfigured(provenance, emitted_at)
        }
    }

    /// The meta block for a failure that happened before the configuration was
    /// resolved: an unparseable command line, or a config file that would not
    /// load.
    pub fn unconfigured(provenance: Option<Provenance>, emitted_at: SystemTime) -> Self {
        Self {
            tool_version: env!("CARGO_PKG_VERSION"),
            // Both read off the one `emitted_at` this run sampled, so a fresh
            // document's two timestamps are the same string by construction
            // rather than by luck (#44). See [`Provenance`].
            fetched_at: provenance.map(|p| format_rfc3339(p.fetched_at(emitted_at))),
            emitted_at: format_rfc3339(emitted_at),
            from_cache: provenance.map(|p| p.from_cache()),
            requested_country: None,
            requested_currency: None,
            requested_storefront: None,
        }
    }
}

/// One `--json` document: the same shape on success and on failure.
///
/// `data` is present exactly when `ok` is true, and `error_type`/`message`
/// exactly when it is false. `ok`, `schema_version` and `meta` are always
/// there, so a consumer can read the version and the provenance off a document
/// before it knows whether the run succeeded.
#[derive(Debug, Clone, Serialize)]
pub struct Envelope {
    pub ok: bool,
    pub schema_version: u32,
    pub meta: Meta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// The stable taxonomy string a caller branches on (#9). See
    /// [`crate::error::ErrorKind`] for the table and the exit codes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Envelope {
    pub fn success(meta: Meta, data: Value) -> Self {
        Self {
            ok: true,
            schema_version: SCHEMA_VERSION,
            meta,
            data: Some(data),
            error_type: None,
            message: None,
        }
    }

    pub fn failure(meta: Meta, kind: ErrorKind, message: String) -> Self {
        Self {
            ok: false,
            schema_version: SCHEMA_VERSION,
            meta,
            data: None,
            error_type: Some(kind.error_type()),
            message: Some(message),
        }
    }

    /// The document, ready to write to stdout, newline-terminated.
    ///
    /// Infallible on purpose: this is the last thing that runs on the failure
    /// path, and a renderer that can itself fail there has no way left to say
    /// so. Only [`Meta`] and two `serde_json::Value`s are serialized here, and
    /// neither can fail — but if serialization ever did, a hand-written
    /// envelope reporting that is still one JSON document on stdout, which is
    /// the contract.
    pub fn render(&self) -> String {
        match serde_json::to_string_pretty(self) {
            Ok(json) => format!("{}\n", json),
            Err(e) => format!(
                "{{\n  \"ok\": false,\n  \"schema_version\": {},\n  \"error_type\": \"json_error\",\n  \"message\": {}\n}}\n",
                SCHEMA_VERSION,
                Value::String(e.to_string())
            ),
        }
    }
}

/// The record fields a section presents.
///
/// The other half of [`ProductView`]'s job: markdown renders a section by
/// printing these, `--json` renders it by keeping them. Declared once here so
/// the two cannot disagree about what `--section nutrition` means.
///
/// Every field of [`ProductDetail`] appears in exactly one section, or in
/// [`ALWAYS_RENDERED`] — pinned by a test, so a field added to the model
/// without being placed here is a failure rather than a field that silently
/// vanishes from every projection.
fn section_fields(section: Section) -> &'static [&'static str] {
    match section {
        Section::Overview => &[
            "brand",
            "price",
            "original_price",
            "currency",
            "rating",
            "review_count",
            "in_stock",
            "product_code",
            "upc",
            "shipping_weight",
            "category_breadcrumb",
        ],
        Section::Description => &["description"],
        Section::Nutrition => &["supplement_facts"],
        Section::Ingredients => &["ingredients"],
        Section::SuggestedUse => &["suggested_use"],
        Section::Warnings => &["warnings"],
        Section::Reviews => &["rating", "review_count", "review_distribution"],
    }
}

/// The fields every `--json` product document carries, whatever `--section`
/// asked for: what the record *is*, and where it came from.
///
/// `extraction` is on this list rather than in a section because provenance is
/// not a section — a projection that could drop it would let a caller hold a
/// record with no way to tell a value nobody read from one the page published,
/// which is the conflation #28 exists to prevent.
pub const ALWAYS_RENDERED: &[&str] = &["name", "product_id", "product_url", "extraction"];

/// A product record as `--json` renders it.
///
/// Two things happen here and nothing else does.
///
/// **`extraction` is replaced by the record's `health()`, verbatim.** The
/// serialized `ProductDetail` carries the raw [`crate::model::Extraction`] —
/// the strategy and the source map — and what a consumer needs is the derived
/// report: the same map plus the absent, defaulted and malformed lists and the
/// `degraded` flag. `serde_json::to_value(product.health())` is that report and
/// this adds nothing to it and computes nothing from it (#28).
///
/// **The keys are narrowed to what `view` asks for.** With no `--section` that
/// is everything. With one it is that section's fields plus
/// [`ALWAYS_RENDERED`] — because a flag that changes what markdown shows and is
/// silently ignored under `--json` is worse than a flag that does nothing.
pub fn format_product_json(
    product: &ProductDetail,
    view: &ProductView,
) -> Result<Value, serde_json::Error> {
    let mut value = serde_json::to_value(product)?;
    let object = value
        .as_object_mut()
        .expect("a ProductDetail serializes as a JSON object");

    object.insert(
        "extraction".to_string(),
        serde_json::to_value(product.health())?,
    );

    if view.requested().is_some() {
        let kept = kept_fields(view);
        object.retain(|key, _| kept.contains(&key.as_str()));
    }

    Ok(value)
}

/// The key names a projected product document keeps.
fn kept_fields(view: &ProductView) -> Vec<&'static str> {
    let mut kept: Vec<&'static str> = ALWAYS_RENDERED.to_vec();
    for section in view.sections() {
        kept.extend_from_slice(section_fields(*section));
    }
    kept
}

/// A search result as `--json` renders it.
///
/// Each card's `extraction` is replaced by that card's `health()`, exactly as
/// [`format_product_json`] does for a product — which is the whole point of
/// #49 reaching here: one provenance shape across both commands, rather than
/// provenance on products and silence on search.
///
/// `--section` does not apply, so nothing is projected away. `total_results`
/// and `fetch` are carried through as the model holds them: `fetch.exhausted`
/// is what tells a caller whether a short result is iHerb running out or the
/// walk stopping (#6), and it is not derivable from the product count.
pub fn format_search_json(result: &SearchResult) -> Result<Value, serde_json::Error> {
    let mut value = serde_json::to_value(result)?;

    if let Some(cards) = value.get_mut("products").and_then(Value::as_array_mut) {
        for (slot, summary) in cards.iter_mut().zip(&result.products) {
            if let Some(object) = slot.as_object_mut() {
                object.insert(
                    "extraction".to_string(),
                    serde_json::to_value(summary.health())?,
                );
            }
        }
    }

    Ok(value)
}

/// Every key a full (unprojected) product document carries. Test support for
/// the claim [`section_fields`] makes about itself.
pub fn product_json_keys(product: &ProductDetail) -> Result<Vec<String>, serde_json::Error> {
    let value = format_product_json(product, &ProductView::everything())?;
    let object: &Map<String, Value> = value
        .as_object()
        .expect("a ProductDetail serializes as a JSON object");
    Ok(object.keys().cloned().collect())
}

/// The keys [`section_fields`] and [`ALWAYS_RENDERED`] account for between
/// them. Test support, as above.
pub fn accounted_json_keys() -> Vec<&'static str> {
    let mut all = ALWAYS_RENDERED.to_vec();
    for section in Section::ALL {
        all.extend_from_slice(section_fields(*section));
    }
    all.sort_unstable();
    all.dedup();
    all
}
