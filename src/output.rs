use crate::cli::Section;
use crate::model::{ExtractionHealth, ProductDetail, SearchResult};
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

        let price_str = format_price(
            product.price,
            product.original_price.as_ref(),
            &product.currency,
        );
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

pub fn format_product_detail(product: &ProductDetail, section: Option<Section>) -> String {
    let mut out = String::new();

    let sections: &[Section] = match section {
        Some(s) => &[s],
        None => Section::ALL,
    };

    if section.is_none() {
        out.push_str(&format!("# {}\n\n", product.name));
    }

    for sec in sections {
        match sec {
            Section::Overview => format_overview(product, &mut out),
            Section::Description => format_description(product, &mut out),
            Section::Nutrition => format_nutrition(product, &mut out),
            Section::Ingredients => {
                // When explicitly requesting ingredients, show supplement facts
                // first (active ingredients) then other ingredients — matching
                // how supplement labels read and what users expect from "what's in it?"
                if section.is_some() {
                    format_nutrition(product, &mut out);
                }
                format_ingredients(product, &mut out);
            }
            Section::SuggestedUse => format_suggested_use(product, &mut out),
            Section::Warnings => format_warnings(product, &mut out),
            Section::Reviews => format_reviews(product, &mut out),
        }
    }

    if out.is_empty() {
        if let Some(sec) = section {
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
        &product.currency,
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
            "- **Data quality:** degraded — no strategy produced {}. Run with --debug for the full provenance table.\n",
            health.fields_absent.join(", ")
        ));
    }

    out.push('\n');
}

fn format_description(product: &ProductDetail, out: &mut String) {
    if let Some(ref desc) = product.description {
        out.push_str("## Description\n");
        out.push_str(desc);
        out.push_str("\n\n");
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

fn format_price(price: f64, original: Option<&f64>, currency: &str) -> String {
    let symbol = match currency {
        "USD" => "$",
        "CHF" => "CHF ",
        "EUR" => "€",
        "GBP" => "£",
        _ => currency,
    };

    match original {
        Some(orig) if *orig > price => {
            let discount = ((*orig - price) / *orig * 100.0).round() as u32;
            format!(
                "{}{:.2} ~~{}{:.2}~~ ({}% off)",
                symbol, price, symbol, orig, discount
            )
        }
        _ => format!("{}{:.2}", symbol, price),
    }
}

pub fn format_cached_at(cached_at: SystemTime) -> String {
    let duration = cached_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs() as i64;

    // Simple date formatting without extra dependencies
    let days = secs / 86400;
    let remaining = secs % 86400;
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;

    // Calculate year/month/day from epoch days
    let mut y = 1970i64;
    let mut d = days;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
        if d < days_in_year {
            break;
        }
        d -= days_in_year;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
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

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC",
        y,
        m + 1,
        d + 1,
        hours,
        minutes
    )
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
///   "sources": { "name": "json_ld", "ingredients": "dom", "warnings": "absent", ... },
///   "fields_absent": ["warnings", "..."],
///   "degraded": false
/// }
/// ```
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
            "yes — a field every product page publishes is missing, so the \
             selectors may have rotted"
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

    out.push_str("\n| Field | Source |\n|---|---|\n");
    for (field, source) in &health.sources {
        out.push_str(&format!("| {} | {:?} |\n", field, source));
    }
    out.push('\n');

    out
}
