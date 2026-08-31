use crate::error::IherbError;
use crate::model::{
    Extraction, Nutrient, ProductDetail, ReviewDistribution, Source, Strategy, SupplementFacts,
};
use chromiumoxide::Page;
use scraper::{Html, Selector};

use super::helpers::{
    debug_dump_html, detect_currency_from_html, extract_text, is_not_found_page, parse_price_str,
    parse_review_count,
};

/// Extract product detail from a page.
///
/// Three strategies, in order: JSON-LD, then JS globals, then DOM selectors.
/// JSON-LD is the one that fires on every captured and freshly-fetched page;
/// the other two are real fallbacks for when it is missing or unparseable.
/// Whichever wins, the result is enriched from the DOM, so field coverage does
/// not depend on which strategy got there first.
pub async fn extract_product(
    page: &Page,
    html: &str,
    product_id: &str,
    base_url: &str,
    currency: &str,
) -> Result<ProductDetail, IherbError> {
    debug_dump_html(html, &format!("product_{}", product_id));

    // Try JSON-LD first (most reliable structured data)
    if let Some(json_ld) = super::extract::extract_json_ld(html) {
        tracing::debug!("Attempting JSON-LD extraction for product {}", product_id);
        if let Some(mut product) = parse_from_json_ld(&json_ld, product_id, base_url) {
            // JSON-LD has core fields; enrich with DOM-only fields
            enrich_from_html(html, &mut product);
            tracing::info!("Successfully extracted product from JSON-LD + DOM enrichment");
            return Ok(product);
        }
        tracing::warn!("JSON-LD extraction failed, trying JS globals");
    }

    // Try JS globals
    if let Ok(Some(globals)) = super::extract::extract_js_globals(page).await {
        tracing::debug!(
            "Attempting JS globals extraction for product {}",
            product_id
        );
        if let Some(mut product) = parse_from_js_globals(&globals, product_id, base_url, currency) {
            enrich_from_html(html, &mut product);
            tracing::info!("Successfully extracted product from JS globals + DOM enrichment");
            return Ok(product);
        }
        tracing::warn!("JS globals extraction failed, falling back to DOM");
    }

    // Fallback to DOM scraping
    tracing::info!("Extracting product from DOM for {}", product_id);
    parse_from_html(html, product_id, base_url, currency)
}

/// Extract price, original price, and currency from JSON-LD offers.
/// Handles both top-level `price`/`priceCurrency` and the `priceSpecification` array.
///
/// The currency is `None` when the offers carry no `priceCurrency` anywhere.
/// The `"USD"` fallback used to live in here, which meant the caller could not
/// tell a currency iHerb published from one this function invented — so
/// provenance recorded the invention as though JSON-LD had supplied it. The
/// fallback now happens at the caller, which records it as
/// [`Source::Defaulted`]. Fixing the *value* is #5; this is only about not
/// vouching for it.
fn extract_prices_from_offers(
    offers: Option<&serde_json::Value>,
) -> (f64, Option<f64>, Option<String>) {
    let offers = match offers {
        Some(o) => o,
        None => return (0.0, None, None),
    };

    // Try top-level offers.price
    let top_price = offers.get("price").and_then(|v| {
        v.as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| v.as_f64())
    });
    let top_currency = offers
        .get("priceCurrency")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if let Some(price) = top_price {
        return (price, None, top_currency);
    }

    // Fall back to priceSpecification array
    if let Some(specs) = offers.get("priceSpecification").and_then(|v| v.as_array()) {
        let mut current_price = None;
        let mut strikethrough_price = None;
        let mut currency = None;

        for spec in specs {
            let spec_price = spec.get("price").and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| v.as_f64())
            });
            let spec_currency = spec
                .get("priceCurrency")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let is_strikethrough = spec
                .get("priceType")
                .and_then(|v| v.as_str())
                .map(|s| s.contains("StrikethroughPrice"))
                .unwrap_or(false);

            if is_strikethrough {
                strikethrough_price = spec_price;
            } else {
                current_price = spec_price;
                if currency.is_none() {
                    currency = spec_currency;
                }
            }
        }

        let price = current_price.unwrap_or(0.0);
        let original = strikethrough_price.filter(|&op| op > price);

        return (price, original, currency.or(top_currency));
    }

    (0.0, None, top_currency)
}

/// Read `window.PRODUCT_DETAILS.availableToPurchase`, which the page writes as
/// the string `"True"` or `"False"`.
///
/// Anything else — an empty string, a shape we have not seen — says nothing and
/// so answers nothing. Reading "not the word true" as `false` is how the
/// fabrications in #30 and #31 got there in the first place.
fn read_available_to_purchase(value: &serde_json::Value) -> Option<bool> {
    if let Some(text) = value.as_str() {
        if text.eq_ignore_ascii_case("true") {
            return Some(true);
        }
        if text.eq_ignore_ascii_case("false") {
            return Some(false);
        }
        return None;
    }
    value.as_bool()
}

/// Read a stock phrase written for a human ("In stock", "Out of stock") into a
/// definite answer, or `None` when the phrase says neither.
fn read_stock_text(text: &str) -> Option<bool> {
    let lower = text.to_lowercase();
    if lower.contains("out of stock") || lower.contains("sold out") {
        Some(false)
    } else if lower.contains("in stock") {
        Some(true)
    } else {
        None
    }
}

/// Read a stock indicator into a definite answer, or `None` when the value is
/// one we have no reading for.
///
/// Two vocabularies say the same thing on an iHerb page and both land here:
/// JSON-LD's schema.org URLs (`https://schema.org/OutOfStock`) and the
/// `window.IHR_DL.product.stckInd` label the page's own JS carries
/// (`InStock`, `OutOfStock`, and `OutOfStockETA`, which a live fetch of
/// product 119174 on 2026-08-31 returned).
///
/// Anything else is `None` rather than a guess. `LimitedAvailability` and
/// `PreOrder` are real schema.org values and neither is a plain yes or no; the
/// old code read them as "not in stock" purely because they do not contain the
/// substring `InStock`.
fn read_stock_indicator(raw: &str) -> Option<bool> {
    // schema.org values arrive as URLs; iHerb's own arrive bare.
    let token = raw.rsplit('/').next().unwrap_or(raw).trim();

    if token.starts_with("OutOfStock") || token == "SoldOut" || token == "Discontinued" {
        Some(false)
    } else if token.starts_with("InStock") {
        Some(true)
    } else {
        None
    }
}

/// Parse product from JSON-LD structured data.
pub fn parse_from_json_ld(
    data: &serde_json::Value,
    product_id: &str,
    base_url: &str,
) -> Option<ProductDetail> {
    let name = data.get("name").and_then(|v| v.as_str())?.to_string();

    if name.is_empty() {
        return None;
    }

    let brand = data
        .get("brand")
        .and_then(|b| {
            b.get("name")
                .and_then(|v| v.as_str())
                .or_else(|| b.as_str())
        })
        .unwrap_or("")
        .to_string();

    let offers = data.get("offers");

    // Try top-level offers.price first, then fall back to priceSpecification
    let (price, original_price, read_currency) = extract_prices_from_offers(offers);

    // The value is unchanged — `"USD"` when the offers name no currency, which
    // is what shipped before and what #5 will fix. What changes is that the
    // fallback is visible here, so the claim below can tell the truth about it.
    const DEFAULT_CURRENCY: &str = "USD";
    let currency = read_currency
        .clone()
        .unwrap_or_else(|| DEFAULT_CURRENCY.to_string());

    let in_stock = offers
        .and_then(|o| o.get("availability"))
        .and_then(|v| v.as_str())
        .and_then(read_stock_indicator);

    let agg = data.get("aggregateRating");
    let rating = agg.and_then(|a| {
        a.get("ratingValue").and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| v.as_f64())
        })
    });
    let review_count = agg.and_then(|a| {
        a.get("reviewCount").and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<u32>().ok())
                .or_else(|| v.as_u64().map(|n| n as u32))
        })
    });

    let description = data
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let product_code = data
        .get("sku")
        .or_else(|| data.get("mpn"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let upc = data
        .get("gtin12")
        .or_else(|| data.get("gtin13"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let read_product_url = data
        .get("url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let product_url = read_product_url
        .clone()
        .unwrap_or_else(|| format!("{}/pr/p/{}", base_url, product_id));

    let mut product = ProductDetail {
        name,
        brand,
        price,
        original_price,
        currency,
        rating,
        review_count,
        product_url,
        product_id: product_id.to_string(),
        in_stock,
        description,
        product_code,
        upc,
        ingredients: None,      // enriched from DOM
        supplement_facts: None, // enriched from DOM
        suggested_use: None,    // enriched from DOM
        warnings: None,         // enriched from DOM
        shipping_weight: None,  // enriched from DOM
        category_breadcrumb: None,
        review_distribution: None, // enriched from DOM
        extraction: Extraction::new(Strategy::JsonLd),
    };

    product.claim_unattributed(Source::JsonLd);

    // Two values JSON-LD did not carry, which `claim_unattributed` would
    // otherwise attribute to it because they are non-empty. No capture takes
    // the currency branch — all five publish `priceCurrency` — and all five
    // take the URL one, because none publishes `url`.
    if read_currency.is_none() {
        product.extraction.reclaim("currency", Source::Defaulted);
    }
    if read_product_url.is_none() {
        product.extraction.reclaim("product_url", Source::Defaulted);
    }

    Some(product)
}

/// Parse product from JS globals (window.PRODUCT_DETAILS, window.IHR_DL).
pub fn parse_from_js_globals(
    globals: &serde_json::Value,
    product_id: &str,
    base_url: &str,
    currency: &str,
) -> Option<ProductDetail> {
    let pd = globals.get("productDetails");
    let ihr = globals.get("ihrProduct");

    // The page writes `prdctNm`. `prdNm` appears on none of the seven captures
    // and on no page fetched live, which is why this rung produced nothing for
    // as long as it existed (#30). `productDetails.name` is kept as the
    // documented second choice even though no capture carries it either.
    let name = ihr
        .and_then(|p| p.get("prdctNm"))
        .or_else(|| pd.and_then(|p| p.get("name")))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if name.is_empty() {
        return None;
    }

    let brand = ihr
        .and_then(|p| p.get("brndNm"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let price_str = ihr
        .and_then(|p| p.get("prc"))
        .and_then(|v| v.as_str())
        .unwrap_or("0");
    let price = parse_price_str(price_str).unwrap_or(0.0);

    let product_code = pd
        .and_then(|p| p.get("code"))
        .or_else(|| ihr.and_then(|p| p.get("prtNum")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // The blob carries a UPC as a JSON number, not a string.
    let upc = ihr.and_then(|p| p.get("upcCd")).and_then(|v| {
        v.as_str()
            .map(|s| s.to_string())
            .or_else(|| v.as_u64().map(|n| n.to_string()))
    });

    // `stckInd` is the product's own answer. `availableToPurchase` is the
    // weaker second opinion — it tracks buyability rather than stock, but the
    // two agree on every page seen so far ("False" on the out-of-stock gummies,
    // "True" on the in-stock Nordic page) and it is better than no answer.
    let in_stock = ihr
        .and_then(|p| p.get("stckInd"))
        .and_then(|v| v.as_str())
        .and_then(read_stock_indicator)
        .or_else(|| {
            pd.and_then(|p| p.get("availableToPurchase"))
                .and_then(read_available_to_purchase)
        });

    // One level only: the blob names a single primary parent category, not a
    // path. It is still the right field — `category_breadcrumb` is a `Vec`
    // because the DOM can supply more, not because this must.
    let category_breadcrumb = ihr
        .and_then(|p| p.get("prmryPrntCtgry"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| vec![s.to_string()]);

    let mut product = ProductDetail {
        name,
        brand,
        price,
        original_price: None,
        currency: currency.to_string(),
        rating: None,
        review_count: None,
        product_url: format!("{}/pr/p/{}", base_url, product_id),
        product_id: product_id.to_string(),
        in_stock,
        description: None,
        product_code,
        upc,
        ingredients: None,
        supplement_facts: None,
        suggested_use: None,
        warnings: None,
        shipping_weight: None,
        category_breadcrumb,
        review_distribution: None,
        extraction: Extraction::new(Strategy::JsGlobals),
    };

    product.claim_unattributed(Source::JsGlobals);

    // The globals carry no currency and no URL at all: the currency is whatever
    // label the caller passed to `--currency` and the URL is synthesised from
    // the id. Both are values nobody read, so neither is attributed to this
    // strategy. `js_globals_parse_when_the_expected_key_is_present` asserting
    // `product.currency == "CHF"` after being handed `"CHF"` is exactly the
    // shape of the problem.
    product.extraction.reclaim("currency", Source::Defaulted);
    product.extraction.reclaim("product_url", Source::Defaulted);

    Some(product)
}

/// Enrich a ProductDetail with fields only available in the DOM (ingredients,
/// supplement facts, etc.)
///
/// Runs on **every** extraction path, so which strategy won does not decide
/// which fields you get (#28). Everything it fills is gap-filling — it never
/// replaces a value a more trusted strategy already produced — with one
/// documented exception in [`enrich_pricing`], which reclaims the field it
/// corrects.
///
/// Every field left filled afterwards that nothing had claimed is attributed to
/// [`Source::Dom`].
pub fn enrich_from_html(html: &str, product: &mut ProductDetail) {
    let doc = Html::parse_document(html);

    if product.brand.is_empty() {
        if let Some(brand) = extract_text(
            &doc,
            "#brand a span bdi, #brand a[data-testid='product-brand-link'] span bdi",
        ) {
            product.brand = brand;
        }
    }

    enrich_pricing(&doc, product);
    enrich_rating_and_reviews(&doc, product);

    // Gap-fill only. A strategy that already read an availability signal has a
    // better one than the DOM: JSON-LD says `OutOfStock` on the gummies page,
    // which carries no `#stock-status` element at all.
    if product.in_stock.is_none() {
        product.in_stock = read_stock_from_dom(&doc, &product.product_id);
    }

    enrich_product_specs(&doc, product);
    parse_overview_sections(html, product);

    if product.supplement_facts.is_none() {
        product.supplement_facts = parse_supplement_facts_html(&doc);
    }
    if product.review_distribution.is_none() {
        product.review_distribution = parse_review_distribution_html(&doc);
    }

    product.claim_unattributed(Source::Dom);
    product.extraction.enriched = true;
}

fn enrich_pricing(doc: &Html, product: &mut ProductDetail) {
    if product.original_price.is_some() && product.price > 0.0 {
        return;
    }
    let sel = match Selector::parse("input#share-email-model") {
        Ok(sel) => sel,
        Err(_) => return,
    };
    let el = match doc.select(&sel).next() {
        Some(el) => el,
        None => return,
    };
    let list_price = el.value().attr("data-list-price").and_then(parse_price_str);
    let disc_price = el
        .value()
        .attr("data-discount-price")
        .and_then(parse_price_str);
    if let (Some(list), Some(disc)) = (list_price, disc_price) {
        if list > disc {
            product.original_price = Some(list);
            if (product.price - list).abs() < 0.01 || product.price == 0.0 {
                // The one place enrichment replaces a value rather than filling
                // a gap: an earlier strategy read the list price as the price.
                // The corrected number came from the DOM, so say so.
                product.price = disc;
                product.extraction.reclaim("price", Source::Dom);
            }
        }
    }
}

fn enrich_rating_and_reviews(doc: &Html, product: &mut ProductDetail) {
    if product.rating.is_none() {
        product.rating = extract_rating_from_stars(doc);
    }
    if product.review_count.is_none() {
        if let Some(text) = extract_text(doc, "a.rating-count span") {
            product.review_count = parse_review_count(&text);
        }
    }
}

fn enrich_product_specs(doc: &Html, product: &mut ProductDetail) {
    if product.shipping_weight.is_none() {
        product.shipping_weight = extract_spec(doc, "Shipping Weight");
    }
    if product.product_code.is_none() {
        product.product_code = extract_spec(doc, "Product Code");
    }
    if product.upc.is_none() {
        product.upc = extract_spec(doc, "UPC");
    }
}

/// Parse structured sections (Suggested Use, Warnings, Ingredients, Description) from product overview.
fn parse_overview_sections(html: &str, product: &mut ProductDetail) {
    let doc = Html::parse_document(html);

    if product.ingredients.is_none() {
        if let Some(text) = extract_text(&doc, ".prodOverviewIngred") {
            product.ingredients = Some(text);
        }
    }

    let h3_sel = match Selector::parse("#product-overview h3") {
        Ok(sel) => sel,
        Err(_) => return,
    };

    for h3 in doc.select(&h3_sel) {
        let heading: String = h3.text().collect::<Vec<_>>().join("").trim().to_lowercase();
        let content = match extract_sibling_div_text(&h3) {
            Some(text) if !text.is_empty() => text,
            _ => continue,
        };
        assign_section_by_heading(&heading, content, product);
    }

    // Genuinely last: the page's own `<meta name="description">`, which is the
    // full description truncated to about 160 characters — the same opening
    // words, cut short. A real description rather than an invention, but a
    // lesser one, so it runs after the heading scan above rather than before
    // it. (It ran before it when first added, which would have let the
    // truncated text pre-empt a real "Description" section on any page that
    // has one. No capture does, so nothing observed it.)
    //
    // It exists so that all three strategies produce the same field coverage
    // for the same page (#28), and `output::format_description` marks what it
    // produces so a reader is not shown a sentence that simply stops.
    //
    // The full text is on the page, under the `#product-overview` "Overview"
    // heading rather than a "Description" one, which is why the scan above
    // never finds it. Reading that markup properly is #13.
    if product.description.is_none() {
        if let Ok(sel) = Selector::parse(r#"meta[name="description"]"#) {
            product.description = doc
                .select(&sel)
                .next()
                .and_then(|el| el.value().attr("content"))
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(|c| c.to_string());
        }
    }
}

/// Extract text content from the first sibling `<div>` after a heading element.
fn extract_sibling_div_text(heading: &scraper::ElementRef) -> Option<String> {
    let mut next = heading.next_sibling();
    while let Some(node) = next {
        if let Some(el) = node.value().as_element() {
            if el.name() == "div" {
                let text: String = node
                    .children()
                    .filter_map(|child| {
                        if let Some(text) = child.value().as_text() {
                            Some(text.to_string())
                        } else if child.value().is_element() {
                            let el_ref = scraper::ElementRef::wrap(child)?;
                            Some(el_ref.text().collect::<Vec<_>>().join(" "))
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim()
                    .to_string();
                return Some(text);
            }
        }
        next = node.next_sibling();
    }
    None
}

fn assign_section_by_heading(heading: &str, content: String, product: &mut ProductDetail) {
    if heading.contains("suggested use") && product.suggested_use.is_none() {
        product.suggested_use = Some(content);
    } else if heading.contains("warning") && product.warnings.is_none() {
        product.warnings = Some(content);
    } else if heading.contains("description") && product.description.is_none() {
        product.description = Some(content);
    }
}

/// Extract a value from #product-specs-list by label prefix.
pub fn extract_spec(doc: &Html, label: &str) -> Option<String> {
    if let Ok(sel) = Selector::parse("#product-specs-list li") {
        for li in doc.select(&sel) {
            let text: String = li.text().collect::<Vec<_>>().join("").trim().to_string();
            if text.starts_with(label) {
                // Extract the value after the label and colon
                let value = text
                    .split_once(':')
                    .map(|(_, v)| v.trim().to_string())
                    .filter(|s| !s.is_empty());
                if value.is_some() {
                    return value;
                }
                // Try extracting from span child
                if let Ok(span_sel) = Selector::parse("span") {
                    if let Some(span) = li.select(&span_sel).next() {
                        let span_text: String =
                            span.text().collect::<Vec<_>>().join("").trim().to_string();
                        if !span_text.is_empty() {
                            return Some(span_text);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Read availability out of the DOM, in preference order, or `None` when the
/// page carries no signal this understands.
///
/// The gummies capture is out of stock and says so four separate ways, and the
/// old code — `!html.contains("Out of Stock")` — ignored every one of them,
/// because the page writes "Out of stock" with a lower-case s (#31).
///
/// The order below is by how specific the signal is to *this* product, which
/// matters more than it looks. `data-is-out-of-stock="True"` appears on the
/// B-Complex page too, on the 30-count variant of an in-stock product; a
/// page-wide substring search for it would report that page out of stock.
/// Every signal here is therefore scoped to an element, and the variant one is
/// scoped to this product's own id.
///
///  1. `#stock-status .stock-status-content strong` — the sentence shown to a
///     human. Present on all four in-stock captures, absent on the gummies.
///  2. `[data-pid="<id>"][data-is-out-of-stock]` — the selected size option.
///     Present on all five captures, and always agrees with JSON-LD.
///  3. `input#modelProperties[data-stock-status]` — a numeric code, `0` on all
///     four in-stock captures and `3` on the gummies. A live fetch of the
///     gummies on 2026-08-31 returned `5` alongside `stckInd: "OutOfStockETA"`.
///     So `0` is in stock and any other value is not; that is an inference from
///     six observations rather than from documentation, which is why it ranks
///     last.
fn read_stock_from_dom(doc: &Html, product_id: &str) -> Option<bool> {
    if let Some(answer) = extract_text(doc, "#stock-status .stock-status-content strong")
        .and_then(|t| read_stock_text(&t))
    {
        return Some(answer);
    }

    let selected_variant = format!("[data-pid=\"{}\"][data-is-out-of-stock]", product_id);
    if let Ok(sel) = Selector::parse(&selected_variant) {
        for el in doc.select(&sel) {
            match el.value().attr("data-is-out-of-stock") {
                Some(v) if v.eq_ignore_ascii_case("true") => return Some(false),
                Some(v) if v.eq_ignore_ascii_case("false") => return Some(true),
                _ => {}
            }
        }
    }

    if let Ok(sel) = Selector::parse("input#modelProperties[data-stock-status]") {
        if let Some(code) = doc
            .select(&sel)
            .next()
            .and_then(|el| el.value().attr("data-stock-status"))
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            return Some(code == "0");
        }
    }

    None
}

/// Fallback: Parse product detail from HTML using CSS selectors.
pub fn parse_from_html(
    html: &str,
    product_id: &str,
    base_url: &str,
    currency: &str,
) -> Result<ProductDetail, IherbError> {
    let doc = Html::parse_document(html);

    if is_not_found_page(html) {
        return Err(IherbError::ProductNotFound(product_id.to_string()));
    }

    let name =
        extract_text(&doc, "h1#name, h1[data-testid='product-name'], h1").unwrap_or_default();

    // No name from any of the three selectors, on a page that did not identify
    // itself as a 404. That is a broken extractor, not a missing product, and
    // the difference matters: `ProductNotFound` tells a caller to stop asking
    // about a perfectly valid id (#28).
    if name.is_empty() {
        return Err(IherbError::ParseFailed(product_id.to_string()));
    }

    let brand = extract_text(
        &doc,
        "#brand a span bdi, #brand a[data-testid='product-brand-link'] span bdi",
    )
    .unwrap_or_default();

    // Price from share-email hidden input (most reliable)
    let (price, original_price) = extract_prices_from_input(&doc).unwrap_or_else(|| {
        let p = extract_text(
            &doc,
            ".purchase-option-one-time .list-price, #product-price .list-price, .price",
        )
        .and_then(|s| parse_price_str(&s))
        .unwrap_or(0.0);
        (p, None)
    });

    // Rating from star title attribute
    let rating = extract_rating_from_stars(&doc);

    // Review count
    let review_count =
        extract_text(&doc, "a.rating-count span").and_then(|s| parse_review_count(&s));

    let in_stock = read_stock_from_dom(&doc, product_id);

    let product_code = extract_spec(&doc, "Product Code");
    let upc = extract_spec(&doc, "UPC");
    let shipping_weight = extract_spec(&doc, "Shipping Weight");

    let supplement_facts = parse_supplement_facts_html(&doc);
    let review_distribution = parse_review_distribution_html(&doc);

    // Detect actual currency from the page, falling back to config currency
    let read_currency = detect_currency_from_html(&doc);
    let detected_currency = read_currency
        .clone()
        .unwrap_or_else(|| currency.to_string());

    let product_url = format!("{}/pr/p/{}", base_url, product_id);

    let mut product = ProductDetail {
        name,
        brand,
        price,
        original_price,
        currency: detected_currency,
        rating,
        review_count,
        product_url,
        product_id: product_id.to_string(),
        in_stock,
        description: None,
        product_code,
        upc,
        ingredients: None,
        supplement_facts,
        suggested_use: None,
        warnings: None,
        shipping_weight,
        category_breadcrumb: None,
        review_distribution,
        extraction: Extraction::new(Strategy::Dom),
    };

    product.claim_unattributed(Source::Dom);

    // The `--currency` label is not a reading of the page. All seven captures
    // publish a currency marker, so none takes this branch; a page that does
    // not is exactly the case #5 is about, and `degraded` now fires on it.
    if read_currency.is_none() {
        product.extraction.reclaim("currency", Source::Defaulted);
    }
    // Always synthesised from the id: the DOM strategy never reads a canonical
    // product URL off the page.
    product.extraction.reclaim("product_url", Source::Defaulted);

    // The DOM strategy enriches from the DOM like every other path does. It
    // reads most of the same elements twice as a result, which is the price of
    // every path producing the same field coverage for the same page (#28).
    // `enrich_from_html` subsumes the `parse_overview_sections` call this used
    // to make, and every other thing it does is gap-filling.
    enrich_from_html(html, &mut product);

    Ok(product)
}

fn extract_prices_from_input(doc: &Html) -> Option<(f64, Option<f64>)> {
    let sel = Selector::parse("input#share-email-model").ok()?;
    let el = doc.select(&sel).next()?;

    let list_price = el.value().attr("data-list-price").and_then(parse_price_str);
    let disc_price = el
        .value()
        .attr("data-discount-price")
        .and_then(parse_price_str);

    match (disc_price, list_price) {
        (Some(disc), Some(list)) if list > disc => Some((disc, Some(list))),
        (Some(disc), _) => Some((disc, None)),
        (None, Some(list)) => Some((list, None)),
        _ => None,
    }
}

fn extract_rating_from_stars(doc: &Html) -> Option<f64> {
    let sel = Selector::parse("a.stars.scroll-to, a.stars").ok()?;
    let el = doc.select(&sel).next()?;
    let title = el.value().attr("title")?;
    // Title format: "4.8/5 - 42,328 Reviews"
    title.split('/').next()?.trim().parse::<f64>().ok()
}

pub fn parse_supplement_facts_html(doc: &Html) -> Option<SupplementFacts> {
    let table_sel =
        Selector::parse(".supplement-facts-container table, table.supplement-facts-table").ok()?;
    let table = doc.select(&table_sel).next()?;

    let row_sel = Selector::parse("tr").ok()?;
    let cell_sel = Selector::parse("td, th").ok()?;

    let mut nutrients = Vec::new();
    let mut serving_size = None;
    let mut servings_per_container = None;

    for row in table.select(&row_sel) {
        let cells: Vec<String> = row
            .select(&cell_sel)
            .map(|c| c.text().collect::<Vec<_>>().join("").trim().to_string())
            .collect();

        // Check for serving size info in merged cells
        if cells.len() == 1 {
            let text = &cells[0];
            let lower = text.to_lowercase();
            if lower.contains("serving size") {
                serving_size = text.split_once(':').map(|(_, v)| v.trim().to_string());
            } else if lower.contains("servings per") {
                servings_per_container = text.split_once(':').map(|(_, v)| v.trim().to_string());
            }
            continue;
        }

        // Skip header rows
        if cells.len() >= 2 {
            let first_lower = cells[0].to_lowercase();
            if first_lower.contains("amount per")
                || first_lower.contains("% daily")
                || first_lower.contains("supplement")
                || first_lower.is_empty()
            {
                continue;
            }
            // Skip dagger footnotes
            if cells[0].starts_with('†') || cells[0].starts_with('*') {
                continue;
            }

            nutrients.push(Nutrient {
                name: cells[0].clone(),
                amount: cells.get(1).cloned().unwrap_or_default(),
                daily_value: cells.get(2).cloned().filter(|s| !s.is_empty()),
            });
        }
    }

    if nutrients.is_empty() && serving_size.is_none() {
        return None;
    }

    Some(SupplementFacts {
        serving_size,
        servings_per_container,
        nutrients,
    })
}

pub fn parse_review_distribution_html(doc: &Html) -> Option<ReviewDistribution> {
    // iHerb uses a <ugc-review-progress-bar> custom element containing
    // a <button class="item"> for each star level (5 down to 1).
    // Each button has:
    //   - a <span> with text like "5 stars"
    //   - a <span> with style="width: XX%;" showing the bar fill
    //   - a <span class="... each-count"> with the raw review count
    // We extract the bar width percentage for each star level.
    let container_sel =
        Selector::parse("ugc-review-progress-bar, .ugc-review-progress-wrap").ok()?;
    let container = doc.select(&container_sel).next()?;

    let button_sel = Selector::parse("button.item").ok()?;
    let buttons: Vec<_> = container.select(&button_sel).collect();
    if buttons.is_empty() {
        return None;
    }

    let mut star_pcts: [Option<f64>; 5] = [None; 5]; // index 0 = 5-star, 4 = 1-star

    for button in &buttons {
        // Find which star level this button represents
        let button_text: String = button.text().collect::<Vec<_>>().join(" ");
        let star_level: Option<usize> = button_text
            .split_whitespace()
            .zip(button_text.split_whitespace().skip(1))
            .find(|(_, second)| second.starts_with("star"))
            .and_then(|(num, _)| num.parse::<usize>().ok())
            .filter(|&n| (1..=5).contains(&n));

        let star_level = match star_level {
            Some(n) => n,
            None => continue,
        };

        // Extract the bar width percentage from the inner <span> style attribute.
        // The bar is: <span class="block h-full bg-green-dark" style="width: 84%;"></span>
        // inside a <div class="percent-wrap ...">
        if let Ok(span_sel) = Selector::parse(".percent-wrap span, span.block") {
            for span in button.select(&span_sel) {
                if let Some(style) = span.value().attr("style") {
                    if let Some(pct) = parse_width_percent(style) {
                        star_pcts[5 - star_level] = Some(pct);
                        break;
                    }
                }
            }
        }
    }

    // Only return if we found at least one star level
    if star_pcts.iter().all(|p| p.is_none()) {
        return None;
    }

    Some(ReviewDistribution {
        five_star: star_pcts[0],
        four_star: star_pcts[1],
        three_star: star_pcts[2],
        two_star: star_pcts[3],
        one_star: star_pcts[4],
    })
}

/// Parse a percentage value from a CSS width style like "width: 84%;".
fn parse_width_percent(style: &str) -> Option<f64> {
    style
        .split(';')
        .filter_map(|prop| {
            let prop = prop.trim();
            if prop.starts_with("width") {
                prop.split(':')
                    .nth(1)
                    .and_then(|v| v.trim().strip_suffix('%'))
                    .and_then(|v| v.trim().parse::<f64>().ok())
            } else {
                None
            }
        })
        .next()
}
