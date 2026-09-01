use crate::app::CacheReport;
use crate::cache::{CacheClearReport, CacheStats};
use crate::cli::Section;
use crate::config::AppConfig;
use crate::error::ErrorKind;
use crate::model::{ExtractionHealth, ProductDetail, SearchResult, Source};
use serde::Serialize;
use serde_json::{Map, Value};
use std::time::Duration as SystemTimeDuration;
use std::time::SystemTime;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

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

        // Only when it is *not* a plain in-stock. Nearly every card in a result
        // set is purchasable, so an unconditional line would be twenty rows of
        // "In Stock" telling a caller nothing — but omitting it entirely reads
        // as in-stock for the rows where that is false, which is #31's
        // fabrication produced by silence (#57). `None` keeps its own wording:
        // no signal on the card said either way, which is not the same claim as
        // out of stock. `--json` carries `in_stock` unconditionally regardless.
        match product.in_stock {
            Some(true) => {}
            Some(false) => out.push_str("- **Availability:** Out of Stock\n"),
            None => out.push_str(
                "- **Availability:** Unknown (no availability signal found on the card)\n",
            ),
        }

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

/// When the data in a document was read, and off what (#7).
///
/// # Why the formatter is handed this at all
///
/// Because the alternative was a `println!` after it. The freshness line used
/// to be appended by `app.rs` once `format_product_detail` had returned, which
/// made it a bullet that belonged to no section: with `--section ingredients`
/// the output was an `## Other Ingredients` block followed by a stray
/// top-level `- **Data from:**`, and with a section the page has no data for it
/// was a bullet under no heading at all. Neither is Markdown a caller can
/// parse, and neither could be fixed from outside the formatter, because
/// *where the line belongs* is a layout decision and the formatter is what
/// makes those.
///
/// It also said the wrong thing. The instant was `SystemTime::now()` — when the
/// document was *printed* — so a 29-day-old cache entry dated itself to the
/// second the command ran. That half is already fixed upstream: the instant
/// here comes from [`Provenance`], which distinguishes a page read during this
/// run from a cache file written three weeks ago (#44).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Freshness {
    fetched_at: SystemTime,
    from_cache: bool,
}

impl Freshness {
    /// What a run reporting itself at `emitted_at` should say about its data.
    pub fn of(provenance: Provenance, emitted_at: SystemTime) -> Self {
        Self {
            fetched_at: provenance.fetched_at(emitted_at),
            from_cache: provenance.from_cache(),
        }
    }

    /// The document footer, set off from the body by a thematic break.
    ///
    /// **Not a list item.** A bullet is a member of whatever list or section it
    /// follows, and this is a statement about the whole document; rendering it
    /// as one is what made `--section` output malformed. A rule and an emphasis
    /// span are the two things Markdown has that mean "this is not part of the
    /// body above".
    ///
    /// The two cases read differently on purpose. "Read from iHerb during this
    /// run" and "read off a file written some weeks ago" are the difference
    /// between a price and a guess, and a reader should not have to subtract
    /// two timestamps to find out which they have.
    fn footer(&self) -> String {
        if self.from_cache {
            format!(
                "\n---\n\n*Data from the local cache, written {}. Nothing was read from \
                 iHerb during this run.*\n",
                format_cached_at(self.fetched_at)
            )
        } else {
            format!(
                "\n---\n\n*Data read from iHerb during this run, at {}.*\n",
                format_cached_at(self.fetched_at)
            )
        }
    }

    /// The sentence that dates an *absence*.
    ///
    /// "No ingredients data available for this product." followed by a
    /// timestamp reads as an absence observed just now, and on a cache hit it is
    /// nothing of the kind: the page was read weeks ago, may have gained the
    /// section since, and nothing in this run went and looked. An absence is the
    /// one claim whose meaning changes with age — a price that is three weeks
    /// old is at least a price that existed — so it says its own age rather than
    /// leaving it to the footer.
    fn as_observed(&self) -> String {
        if self.from_cache {
            format!(
                " That is what the cached record says, and it was read on {} — \
                 the page may have gained one since, and nothing in this run went \
                 back to look.",
                format_cached_at(self.fetched_at)
            )
        } else {
            " The page was read during this run and published none.".to_string()
        }
    }
}

/// The whole Markdown document a `product` invocation prints.
///
/// Body, then the provenance table if `--debug` asked for it, then the
/// freshness footer — in that order, and assembled here rather than in
/// `app.rs`, so there is one place that knows what a finished document looks
/// like. See [`Freshness`] for what the caller used to do instead.
pub fn format_product_document(
    product: &ProductDetail,
    view: &ProductView,
    freshness: Freshness,
    with_extraction_health: bool,
) -> String {
    let mut out = format_product_detail(product, view, freshness);

    // The provenance table is reachable from a caller, on demand. `--json`
    // carries the same block unconditionally, because there it costs a consumer
    // nothing to ignore and costs it everything to be unable to ask.
    if with_extraction_health {
        out.push_str(&format!(
            "\n{}",
            format_extraction_health(&product.health())
        ));
    }

    close_document(out, freshness)
}

/// The whole Markdown document a `search` invocation prints.
pub fn format_search_document(result: &SearchResult, limit: usize, freshness: Freshness) -> String {
    let mut out = format_search_results(result);

    // `--limit` counts distinct products, so falling short of it is a fact
    // about the fetch and has to be said out loud (#6, #33). A caller counting
    // rows cannot otherwise tell "iHerb has no more" from "we stopped walking".
    if let Some(note) = format_search_shortfall(result, limit) {
        out.push_str(&format!("\n{}", note));
    }

    close_document(out, freshness)
}

/// Trim the body's trailing blank lines and stamp the footer on.
///
/// The trim is not cosmetic. Sections end with a blank line so they stack
/// legibly, and a footer appended after one leaves two blank lines before the
/// rule — enough that some renderers stop treating the rule as a break from the
/// paragraph above it.
fn close_document(mut out: String, freshness: Freshness) -> String {
    let body = out.trim_end().len();
    out.truncate(body);
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&freshness.footer());
    out
}

/// The body of a product document: the sections `--section` resolved to, and
/// nothing else.
///
/// `freshness` reaches only one line here — the one that reports an *absence*,
/// whose meaning depends on when it was observed. The footer is
/// [`format_product_document`]'s.
pub fn format_product_detail(
    product: &ProductDetail,
    view: &ProductView,
    freshness: Freshness,
) -> String {
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
            // The absence is dated. See [`Freshness::as_observed`]: a cached
            // "no ingredients" is a claim about a page nobody looked at during
            // this run.
            out.push_str(&format!(
                "No {} data available for this product.{}\n",
                sec.label(),
                freshness.as_observed()
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

/// An instant as a human reads it: `2026-09-01 12:34 UTC`.
///
/// Minute resolution, because that is what a reader wants off a footer. The
/// machine-readable rendering is [`format_rfc3339`], and the two must not
/// disagree about which day it is — which is why they share one
/// [`OffsetDateTime`] conversion rather than two arithmetics.
pub fn format_cached_at(cached_at: SystemTime) -> String {
    let t = OffsetDateTime::from(cached_at);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute()
    )
}

/// The same instant as RFC 3339 in UTC, e.g. `2026-08-31T09:14:22Z`.
///
/// What `--json`'s envelope carries (#44). The human line above rounds to the
/// minute, which is the right resolution to read and the wrong one to store: a
/// consumer comparing two records needs the seconds, and needs a format it can
/// parse rather than one it has to recognise.
///
/// Seconds and no finer, deliberately. A cache entry's instant is a file
/// mtime, which carries nanoseconds on every filesystem this runs on, and
/// `Rfc3339` renders a non-zero nanosecond as a fractional part. That would put
/// two different shapes of timestamp in one field depending on where the
/// instant came from — a fresh record's clock sample against a cached record's
/// mtime — for a precision nothing in this tool has a use for.
pub fn format_rfc3339(at: SystemTime) -> String {
    OffsetDateTime::from(at)
        .replace_nanosecond(0)
        .expect("0 is a valid nanosecond")
        .format(&Rfc3339)
        .expect("a UTC OffsetDateTime always formats as RFC 3339")
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
// `cache` (#22)
// ---------------------------------------------------------------------------

/// The Markdown a `cache` invocation prints.
///
/// No freshness footer. The other two commands report data that was read at
/// some instant, possibly weeks ago; this one describes the cache directory as
/// it is right now, and stamping a "Data from" on it would be dating a fetch
/// that never happened.
pub fn format_cache_report(report: &CacheReport, config: &AppConfig) -> String {
    match report {
        // Bare, and no heading: `cache path` exists to be substituted into
        // another command. `$(iherb-cli cache path)` should be the path.
        CacheReport::Path { dir } => format!("{}\n", dir.display()),
        CacheReport::Stats(stats) => format_cache_stats(stats, config),
        CacheReport::Cleared(cleared) => format_cache_cleared(cleared),
    }
}

fn format_cache_stats(stats: &CacheStats, config: &AppConfig) -> String {
    let mut out = String::from("## Cache\n");
    out.push_str(&format!("- **Path:** {}\n", stats.dir.display()));
    out.push_str(&format!(
        "- **Entries:** {}\n",
        format_number_usize(stats.entries)
    ));
    out.push_str(&format!("- **Size:** {}\n", format_bytes(stats.bytes)));
    // Both `None` exactly when the cache is empty, which is a fact rather than
    // a missing value — an empty cache has no oldest entry.
    out.push_str(&format!(
        "- **Oldest entry:** {}\n",
        stats
            .oldest
            .map(format_cached_at)
            .unwrap_or_else(|| "none — the cache is empty".to_string())
    ));
    out.push_str(&format!(
        "- **Newest entry:** {}\n",
        stats
            .newest
            .map(format_cached_at)
            .unwrap_or_else(|| "none — the cache is empty".to_string())
    ));
    out.push_str(&format!(
        "- **TTL:** {} — an entry older than this is a miss and gets refetched.\n",
        format_duration(config.cache_ttl)
    ));
    out
}

fn format_cache_cleared(cleared: &CacheClearReport) -> String {
    let mut out = String::from("## Cache cleared\n");
    out.push_str(&format!("- **Path:** {}\n", cleared.dir.display()));
    out.push_str(&format!(
        "- **Removed:** {}, {}\n",
        pluralize_entries(cleared.removed.len()),
        format_bytes(cleared.removed_bytes)
    ));
    out.push_str(&format!(
        "- **Kept:** {}\n",
        pluralize_entries(cleared.kept)
    ));

    // Said out loud rather than folded into `kept`. "Cleared the Norwegian
    // cache" while leaving the Norwegian search results in place is exactly the
    // half-truth a caller acts on.
    if cleared.unattributable > 0 {
        out.push_str(&format!(
            "- **Could not be attributed to a country:** {} search {}, kept. A search \
             entry is named by a hash of the whole request, so the country is inside \
             the name and cannot be read off it. Clear them with `--older-than` or \
             `--all`.\n",
            format_number_usize(cleared.unattributable),
            if cleared.unattributable == 1 {
                "entry"
            } else {
                "entries"
            }
        ));
    }
    if !cleared.failed.is_empty() {
        out.push_str(&format!(
            "- **Could not be removed:** {}\n",
            cleared.failed.join("; ")
        ));
    }
    out
}

/// The same report as one JSON document, in the same envelope as everything
/// else (#22, #44).
pub fn format_cache_json(
    report: &CacheReport,
    config: &AppConfig,
) -> Result<Value, serde_json::Error> {
    let value = match report {
        CacheReport::Path { dir } => serde_json::json!({
            "path": dir.display().to_string(),
        }),
        CacheReport::Stats(stats) => serde_json::json!({
            "path": stats.dir.display().to_string(),
            "entries": stats.entries,
            "bytes": stats.bytes,
            // RFC 3339, like every other machine-readable instant this tool
            // emits, and `null` for an empty cache because an empty cache has
            // no oldest entry.
            "oldest": stats.oldest.map(format_rfc3339),
            "newest": stats.newest.map(format_rfc3339),
            "ttl_seconds": config.cache_ttl.as_secs(),
        }),
        CacheReport::Cleared(cleared) => serde_json::json!({
            "path": cleared.dir.display().to_string(),
            "removed": cleared.removed,
            "removed_count": cleared.removed.len(),
            "removed_bytes": cleared.removed_bytes,
            "kept": cleared.kept,
            "unattributable": cleared.unattributable,
            "failed": cleared.failed,
        }),
    };
    Ok(value)
}

/// A byte count as a reader wants it, in SI units.
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

/// A duration in the same spelling `--cache-ttl` accepts, so what is printed
/// can be pasted back in.
fn format_duration(d: SystemTimeDuration) -> String {
    let secs = d.as_secs();
    for (unit, size) in [("w", 604_800), ("d", 86_400), ("h", 3_600), ("m", 60)] {
        if secs >= size && secs.is_multiple_of(size) {
            return format!("{}{}", secs / size, unit);
        }
    }
    format!("{}s", secs)
}

fn format_number_usize(n: usize) -> String {
    format_number(u32::try_from(n).unwrap_or(u32::MAX))
}

/// `1 entry`, `0 entries`. A count a person reads, not a template.
fn pluralize_entries(n: usize) -> String {
    format!(
        "{} {}",
        format_number_usize(n),
        if n == 1 { "entry" } else { "entries" }
    )
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
