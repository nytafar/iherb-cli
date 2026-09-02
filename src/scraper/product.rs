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
        if let Some(mut product) = parse_from_js_globals(&globals, product_id, base_url) {
            enrich_from_html(html, &mut product);
            tracing::info!("Successfully extracted product from JS globals + DOM enrichment");
            return Ok(product);
        }
        tracing::warn!("JS globals extraction failed, falling back to DOM");
    }

    // Fallback to DOM scraping
    tracing::info!("Extracting product from DOM for {}", product_id);
    parse_from_html(html, product_id, base_url)
}

/// Extract price, original price, and currency from JSON-LD offers.
/// Handles both top-level `price`/`priceCurrency` and the `priceSpecification` array.
///
/// The currency is `None` when the offers carry no `priceCurrency` anywhere.
/// The `"USD"` fallback used to live in here, which meant the caller could not
/// tell a currency iHerb published from one this function invented — so
/// provenance recorded the invention as though JSON-LD had supplied it. #49
/// moved the fallback out to the caller and recorded it as
/// [`Source::Defaulted`]; #5 deleted it. `None` now travels all the way to
/// [`ProductDetail::currency`] rather than being turned into a value on the
/// way.
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
        currency: read_currency,
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

    // One value JSON-LD did not carry, which `claim_unattributed` would
    // otherwise attribute to it because it is non-empty: all five captures take
    // this branch, because none publishes `url`. The currency needs no such
    // rescue any more — offers with no `priceCurrency` now leave the field
    // `None`, which `claim_unattributed` skips and `source_of` reports as
    // [`Source::Absent`].
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
        // The globals carry no currency at all. This used to be the
        // `--currency` label, which made a US price read as a Swiss one on any
        // page that reached this strategy (#5).
        currency: None,
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

    // The globals carry no URL either: it is synthesised from the id, so it is
    // a value nobody read and is not attributed to this strategy.
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
        record_review_distribution(parse_review_distribution_html(&doc), product);
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

/// Every `label: value` row in `#product-specs-list`, in page order.
///
/// The list is a flat `<ul>` of `<li>Label: <span>value</span></li>` rows —
/// `First available`, `Shipping weight`, `Product code`, `UPC`,
/// `Package quantity` and `Dimensions` on all five captures. Parsing the whole
/// list once and looking a label up in the result replaces the old
/// label-at-a-time scan, and is what makes bounding the value possible: the
/// `Shipping weight` row also contains the info tooltip, which is not part of
/// the value (#2).
pub fn parse_product_specs(doc: &Html) -> Vec<(String, String)> {
    let Ok(row_sel) = Selector::parse("#product-specs-list li") else {
        return Vec::new();
    };
    // The "what does shipping weight mean" popover, which lives *inside* the
    // `Shipping weight` row. It is page chrome, not the value, and the old
    // "everything after the first colon" rule swallowed all 500 words of it.
    let Ok(tooltip_sel) = Selector::parse("#cms-popover-tooltip, cms-popover") else {
        return Vec::new();
    };

    doc.select(&row_sel)
        .filter_map(|li| parse_spec_row(li, &tooltip_sel))
        .collect()
}

/// Split one `<li>` into its label and its value.
///
/// Walks the row's own children in document order rather than taking
/// `li.text()` wholesale, because the row is not just text: the colon that
/// separates label from value is in a text node, the value is in one or more
/// `<span>`s, and the tooltip is a subtree that has to be skipped rather than
/// read. `Dimensions` is why the text nodes *between* spans are kept — the
/// `", "` joining `5.85 x 3.2 x 3.15 in` to `0.72 lb` is the page's own.
fn parse_spec_row<'a>(
    li: scraper::ElementRef<'a>,
    tooltip_sel: &Selector,
) -> Option<(String, String)> {
    let mut label = String::new();
    let mut value = String::new();
    let mut seen_colon = false;

    for child in li.children() {
        if let Some(text) = child.value().as_text() {
            if seen_colon {
                value.push_str(text);
            } else if let Some((before, after)) = text.split_once(':') {
                label.push_str(before);
                value.push_str(after);
                seen_colon = true;
            } else {
                label.push_str(text);
            }
            continue;
        }
        let Some(el) = scraper::ElementRef::wrap(child) else {
            continue;
        };
        if tooltip_sel.matches(&el) {
            continue;
        }
        let text: String = el.text().collect();
        if seen_colon {
            value.push_str(&text);
        } else {
            label.push_str(&text);
        }
    }

    if !seen_colon {
        return None;
    }
    let value = squeeze(&value);
    if value.is_empty() {
        return None;
    }
    Some((squeeze(&label), value))
}

/// Collapse the HTML source's line breaks and indentation into single spaces.
///
/// The `Dimensions` row writes its separating comma on its own line on three of
/// the five captures, so the space that would otherwise be left in front of it
/// is indentation rather than content and goes too.
fn squeeze(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() && !word.starts_with(',') {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Look one label up in `#product-specs-list`.
///
/// The match is case-insensitive because the page writes `Product code:` and
/// `Shipping weight:` in sentence case while the call sites ask in title case,
/// so two of the three production lookups could never resolve (#2). It is also
/// whole-label rather than the old `starts_with`: a prefix match would let
/// `"Product"` answer with the product code, and every real caller names a
/// whole label anyway.
pub fn extract_spec(doc: &Html, label: &str) -> Option<String> {
    parse_product_specs(doc)
        .into_iter()
        .find(|(found, _)| found.eq_ignore_ascii_case(label))
        .map(|(_, value)| value)
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

    // What the page says its prices are in, or nothing. There is no fallback:
    // the `--currency` label used to fill this, so a page whose currency
    // markers we could not read had the caller's guess printed against iHerb's
    // numbers (#5).
    let read_currency = detect_currency_from_html(&doc);
    let currency_source = read_currency.source();

    let product_url = format!("{}/pr/p/{}", base_url, product_id);

    let mut product = ProductDetail {
        name,
        brand,
        price,
        original_price,
        currency: read_currency.value(),
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
        // Left to the `enrich_from_html` call at the end of this function,
        // which every path makes. Reading it here too would mean two places
        // deciding whether a hydrated-but-unreadable widget is `Malformed`, and
        // the enrichment pass is the one that also records the provenance.
        review_distribution: None,
        extraction: Extraction::new(Strategy::Dom),
    };

    product.claim_unattributed(Source::Dom);

    // Always synthesised from the id: the DOM strategy never reads a canonical
    // product URL off the page.
    product.extraction.reclaim("product_url", Source::Defaulted);

    // Reclaimed even when the page named its currency, because `read_currency`
    // is the only thing that knows *how well* it named it. A page whose price
    // starts with a bare `$` carries no currency and is `Source::Malformed`,
    // not `Source::Absent`: a signal was on the page and could not be resolved,
    // and `claim_unattributed` would file the resulting `None` as ordinary
    // absence (#52).
    product.extraction.reclaim("currency", currency_source);

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

/// Whether a merged Supplement Facts row is the servings-per-container line
/// (#54).
///
/// The match used to be `contains("servings per")`, plural, and iHerb is not
/// consistent about it: some labels print `Servings Per Container`, some print
/// `Serving Per Container`. On the singular pages the row was read past and
/// `servings_per_container` came back `None` for a page that had stated it —
/// two of the twelve captures in this repository, and both of them recovered
/// by this.
///
/// It was invisible until #8 captured a corpus wide enough to contain a
/// counterexample: every page here before that was a US supplement that
/// happened to spell it plural, so the rule was one no available page could
/// break.
///
/// # This does not make a missing row into a present one
///
/// The two halves both have to be there, and `"per container"` is the half that
/// carries it: a page that never mentions a per-container count matches
/// nothing here and still answers `None`, which is the right answer and a
/// different fact from this bug. Five of the twenty-five products in a recent
/// ranking were that case — NOW Foods publishes a serving size and no serving
/// count on product 692 at all — and deriving a count for them is #40's
/// problem, from the units in the title, not this one's.
fn is_servings_per_container_row(lower: &str) -> bool {
    lower.contains("serving") && lower.contains("per container")
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
            } else if is_servings_per_container_row(&lower) {
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

/// File a histogram read onto the record: the value if there was one, and the
/// provenance either way.
///
/// [`HistogramRead::Malformed`] is the whole point. It leaves
/// `review_distribution` empty — there is genuinely nothing to put there, and
/// inventing a bar would be the bug this codebase exists to prevent — but it
/// claims the field as [`Source::Malformed`], so `health()` reports rot rather
/// than the absence it is otherwise indistinguishable from (#28, #32).
///
/// `Absent` and `NotHydrated` claim nothing, which leaves the field
/// [`Source::Absent`]: both are the page having no histogram, which is a real
/// answer about a product and not a failure of ours.
fn record_review_distribution(read: HistogramRead, product: &mut ProductDetail) {
    match read {
        HistogramRead::Read(dist) => product.review_distribution = Some(dist),
        HistogramRead::Malformed(fault) => {
            tracing::warn!(
                "review histogram for product {} is hydrated but unreadable: {}",
                product.product_id,
                fault
            );
            product
                .extraction
                .claim("review_distribution", Source::Malformed);
        }
        HistogramRead::Absent | HistogramRead::NotHydrated => {}
    }
}

/// What reading the review histogram found. Four outcomes, because three of
/// them used to be `None` and a caller could not tell them apart (#28, #32).
///
/// The one that matters is [`HistogramRead::Malformed`]. "The widget was there
/// and we could not read it" is rot; "the page has no widget" is a page with no
/// reviews. Reporting both as absence is how a broken selector stays invisible
/// until someone happens to re-fetch a page and notice the section missing.
#[derive(Debug, Clone, PartialEq)]
pub enum HistogramRead {
    /// No `<ugc-review-progress-bar>` anywhere on the page. Two captures.
    Absent,
    /// The element is present and holds no bars — the 68-byte shell two
    /// captures carry, where the widget had not filled in before capture.
    /// Absence of data, not a failure to read it.
    NotHydrated,
    /// Bars were present and were read.
    Read(ReviewDistribution),
    /// Bars were present and could not be read. Rot.
    Malformed(HistogramFault),
}

/// Why a hydrated histogram could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistogramFault {
    /// Bars are there and not one of them names a star level in either of the
    /// two ways this understands. The likeliest cause is the star glyph being
    /// redrawn: see [`star_level_from_glyphs`].
    NoBarNamesItsLevel,
    /// Two bars resolved to the same star level. Whichever reading produced
    /// that is wrong, and there is no way to tell which bar is which — so no
    /// bar from this widget can be trusted, not just the colliding pair.
    DuplicateLevel,
    /// Bars name their levels and not one carries a width to read. The bar's
    /// markup has changed even though the star glyph has not.
    NoBarCarriesAWidth,
}

impl std::fmt::Display for HistogramFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::NoBarNamesItsLevel => "no bar names its star level",
            Self::DuplicateLevel => "two bars claim the same star level",
            Self::NoBarCarriesAWidth => "no bar carries a width to read",
        };
        f.write_str(reason)
    }
}

/// Read the review histogram out of iHerb's `<ugc-review-progress-bar>`.
///
/// The widget is a `<button class="item">` per star level, high to low, each
/// holding the star level it stands for, a bar whose CSS width is the
/// percentage, and an `each-count` span with the raw count.
///
/// # Two ways a button names its star level
///
/// This used to look only for the words `5 stars` in the button's text, and on
/// the one capture whose widget is hydrated there is no such text: the level is
/// drawn as a `<ugc-star>` full of SVG. Every bar was skipped and the function
/// returned nothing (#32). Both readings are tried now, text first:
///
///  1. `"5 stars"` in the button's own text — the shape the widget's markup
///     described, and the one a non-JS render would produce.
///  2. How many of the button's five star glyphs are drawn filled. See
///     [`star_level_from_glyphs`] for what "filled" is read from, and why it is
///     no longer the colour.
///
/// # What it refuses to answer
///
/// A page with no widget is [`HistogramRead::Absent`] and an unhydrated shell
/// is [`HistogramRead::NotHydrated`]; neither is a distribution of zeroes.
///
/// A hydrated widget this cannot read is [`HistogramRead::Malformed`] — never
/// absence. That covers a widget whose bars name no level, one whose bars
/// collide on a level, and one whose bars carry no width. Every marker below is
/// read off how the widget is *drawn*, because iHerb gives the buttons no aria
/// label, no data attribute and no per-level class to read instead; so every
/// marker can rot, and this is what makes the rot say so out loud.
///
/// A widget that yields *some* bars is [`HistogramRead::Read`], with `None` in
/// the buckets no bar filled. That is not a partial failure being swallowed: a
/// star level with no reviews may legitimately have no bar, and `None` already
/// means "unknown" in every bucket of [`ReviewDistribution`].
///
/// # How far the evidence goes
///
/// One captured page has a hydrated widget. That the glyph reading is how iHerb
/// encodes the level on *every* live page is an inference from that single
/// sample, not an established fact — which is why the text reading is kept
/// rather than replaced.
pub fn parse_review_distribution_html(doc: &Html) -> HistogramRead {
    let Ok(container_sel) = Selector::parse("ugc-review-progress-bar, .ugc-review-progress-wrap")
    else {
        return HistogramRead::Absent;
    };
    let Some(container) = doc.select(&container_sel).next() else {
        return HistogramRead::Absent;
    };

    let (Ok(button_sel), Ok(bar_sel)) = (
        Selector::parse("button.item"),
        Selector::parse(".percent-wrap span, span.block"),
    ) else {
        return HistogramRead::Absent;
    };

    let buttons: Vec<_> = container.select(&button_sel).collect();
    if buttons.is_empty() {
        return HistogramRead::NotHydrated;
    }

    let mut star_pcts: [Option<f64>; 5] = [None; 5]; // index 0 = 5-star, 4 = 1-star
    let mut named_a_level = false;

    for button in buttons {
        let Some(star_level) = read_star_label(&button).or_else(|| star_level_from_glyphs(&button))
        else {
            continue;
        };
        named_a_level = true;

        let slot = &mut star_pcts[5 - star_level];
        if slot.is_some() {
            return HistogramRead::Malformed(HistogramFault::DuplicateLevel);
        }

        let Some(pct) = button
            .select(&bar_sel)
            .filter_map(|span| span.value().attr("style"))
            .find_map(parse_width_percent)
        else {
            continue;
        };
        *slot = Some(pct);
    }

    if !named_a_level {
        return HistogramRead::Malformed(HistogramFault::NoBarNamesItsLevel);
    }
    if star_pcts.iter().all(|p| p.is_none()) {
        return HistogramRead::Malformed(HistogramFault::NoBarCarriesAWidth);
    }

    HistogramRead::Read(ReviewDistribution {
        five_star: star_pcts[0],
        four_star: star_pcts[1],
        three_star: star_pcts[2],
        two_star: star_pcts[3],
        one_star: star_pcts[4],
    })
}

/// The star level a bar stands for, counted off its star glyphs.
///
/// Each button holds a `<ugc-star>` of five `<li class="ugc-star-item">`, and
/// the level is how many of them are drawn filled: the five buttons on
/// product-104996 hold five, four, three, two and one.
///
/// # Why not the colour
///
/// This keyed on `path[fill="#FAC627"]` when #32 first landed, and review was
/// right that a hardcoded brand colour is a rot waiting to happen: a re-theme
/// would silently empty the histogram.
///
/// There is nothing semantic to use instead. Every button on the captured page
/// is attribute-identical — no `aria-label`, no `data-*`, no per-level class,
/// and no JSON anywhere on the page carrying the counts (the widget's own
/// `10,434` appears exactly once in 4 MB of HTML). What does distinguish the
/// glyphs is *structure*: iHerb draws an empty star as a ground layer plus an
/// outline, and a filled one by inserting a fill layer between them. So a
/// filled star carries two painted `<path>`s and an empty one carries a single
/// one, whatever colours those paths are given.
///
/// That is colour-blind — a re-theme of the gold or of the ground keeps the
/// count at two — but it is still read off how the widget is drawn, so it can
/// rot too. What makes that safe is the caller: any rot severe enough to break
/// this reading makes every button resolve to the same level or to none, and
/// both are reported as [`HistogramRead::Malformed`] rather than as absence.
fn star_level_from_glyphs(button: &scraper::ElementRef<'_>) -> Option<usize> {
    let (Ok(star_sel), Ok(painted_sel)) = (
        Selector::parse("li.ugc-star-item"),
        Selector::parse("path[fill]"),
    ) else {
        return None;
    };

    let filled = button
        .select(&star_sel)
        .filter(|star| {
            star.select(&painted_sel)
                .filter(|path| path.value().attr("fill") != Some("none"))
                .count()
                >= 2
        })
        .count();

    (1..=5).contains(&filled).then_some(filled)
}

/// The star level written out in the button's text, as in `5 stars`.
fn read_star_label(button: &scraper::ElementRef<'_>) -> Option<usize> {
    let text: String = button.text().collect::<Vec<_>>().join(" ");
    let words: Vec<&str> = text.split_whitespace().collect();
    words
        .windows(2)
        .find(|pair| pair[1].starts_with("star"))
        .and_then(|pair| pair[0].parse::<usize>().ok())
        .filter(|n| (1..=5).contains(n))
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
