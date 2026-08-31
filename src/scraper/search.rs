use crate::cli::SortOrder;
use crate::error::IherbError;
use crate::model::{ProductSummary, SearchResult};
use chromiumoxide::Page;
use scraper::{Html, Selector};

use super::helpers::{
    debug_dump_html, detect_currency_from_html, extract_element_text, parse_price_str,
    parse_review_count,
};

const RESULTS_PER_PAGE: usize = 48;

/// A category the search can be narrowed to: one of iHerb's numeric category
/// ids, which is the only thing its `cids` parameter accepts.
///
/// A newtype rather than a `String` so a slug cannot reach the URL builder.
/// That is exactly what #4 was: `--category supplements` produced
/// `cids=supplements`, which iHerb ignores, so the search silently returned
/// everything while the caller believed it had filtered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryId(String);

impl CategoryId {
    /// Resolve a `--category` argument to a category id.
    ///
    /// A numeric id is taken as given — the site's own facet links carry those,
    /// and any id we do not know a name for still works. A name is looked up in
    /// [`CATEGORY_ALIASES`]. Anything else is an error: the one thing this must
    /// not do is pass the argument through and let the search quietly ignore it.
    pub fn resolve(input: &str) -> anyhow::Result<Self> {
        let trimmed = input.trim();
        if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
            return Ok(Self(trimmed.to_string()));
        }

        let wanted = trimmed.to_ascii_lowercase();
        if let Some((_, id)) = CATEGORY_ALIASES.iter().find(|(slug, _)| *slug == wanted) {
            return Ok(Self((*id).to_string()));
        }

        anyhow::bail!(
            "Unknown --category {:?}. Use a numeric iHerb category id, or one of: {}",
            input,
            CATEGORY_ALIASES
                .iter()
                .map(|(slug, _)| *slug)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CategoryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Names for the categories iHerb's own pages name, `(slug, cids)`.
///
/// **Every id here was read off a captured page**, not looked up or guessed:
/// the departments come from the category facet on `search-vitamin-c`, the
/// supplement categories from the same facet on `category-supplements`, and
/// `tests/parsers/search.rs` checks each row against those pages. The slugs are
/// ours — iHerb publishes a title, not a slug, for these — and are derived from
/// the title mechanically: any parenthesised gloss dropped, apostrophes dropped
/// outright, everything else that is not a letter or a digit hyphenated,
/// lowercased. `slugify` in `tests/parsers/search.rs` is that rule written out,
/// and it is checked against every row. The naming is the one invented thing in this
/// table, and it is a name for the CLI rather than a claim about the site.
///
/// It is an alias table, not a catalogue: a category with no row here is still
/// reachable by its numeric id. #21's `catalog` command is what replaces
/// looking ids up by hand.
///
/// One name is deliberately missing. The nav on both captures links
/// `/c/mushrooms?cids=101022`, while the category facet calls 100945
/// "Mushrooms" — two ids, one name, and no capture says which one `--category
/// mushrooms` should mean. Guessing would be the same class of mistake as #4
/// itself, so `mushrooms` resolves to neither and both ids still work.
pub const CATEGORY_ALIASES: &[(&str, &str)] = &[
    // Departments: the top level of the category facet, parent id 1475.
    ("supplements", "1855"),
    ("sports", "101046"),
    ("baby-kids", "2089"),
    ("beauty", "100483"),
    ("bath-personal-care", "100477"),
    ("grocery", "2992"),
    ("home", "2203"),
    ("pets", "2236"),
    ("gifts", "100529"),
    // Under Supplements (1855).
    ("herbs", "2282"),
    ("vitamins", "101072"),
    ("gut-health", "8736"),
    ("brain-cognitive", "105803"),
    ("minerals", "1800"),
    ("antioxidants", "1476"),
    ("bone-joint-cartilage", "100727"),
    ("amino-acids", "1694"),
    ("childrens-health", "100349"),
    ("sleep", "8738"),
    ("greens-superfoods", "100858"),
    ("womens-health", "8741"),
    ("protein", "101005"),
    ("weight-management", "100804"),
    ("omegas-fish-oils", "1542"),
    ("hair-skin-nails", "100861"),
    ("mens-health", "3282"),
    ("detox-cleanse-formulas", "100800"),
    ("eye-ear-nose", "100821"),
    ("bee-products", "1930"),
    ("phospholipids", "102094"),
    ("organ-meats", "107073"),
];

pub fn build_search_url(
    base_url: &str,
    query: &str,
    sort: SortOrder,
    category: Option<&CategoryId>,
    page_num: usize,
) -> String {
    let sort_param = sort.as_url_param();

    let category_param = match category {
        Some(cat) => format!("&cids={}", cat),
        None => String::new(),
    };

    let page_param = if page_num > 1 {
        format!("&p={}", page_num)
    } else {
        String::new()
    };

    format!(
        "{}/search?kw={}{}{}{}",
        base_url,
        urlencoded(query),
        sort_param,
        category_param,
        page_param
    )
}

fn urlencoded(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Extract search results from a page.
///
/// There is exactly one strategy: read the product cards out of the DOM.
/// `_page` is unused today and kept so a future strategy that has to evaluate
/// JS on the live page (a `__NEXT_DATA__`-style blob, an XHR payload) can be
/// added without changing every caller.
pub async fn extract_search(
    _page: &Page,
    html: &str,
    query: &str,
    base_url: &str,
    currency: &str,
) -> Result<SearchResult, IherbError> {
    debug_dump_html(html, &format!("search_{}", query.replace(' ', "_")));

    tracing::info!("Extracting search results from DOM");
    parse_search_from_html(html, query, base_url, currency)
}

/// Parse search results from HTML using data attributes and CSS selectors.
pub fn parse_search_from_html(
    html: &str,
    query: &str,
    base_url: &str,
    currency: &str,
) -> Result<SearchResult, IherbError> {
    let doc = Html::parse_document(html);
    let total_results = extract_total_results(&doc);
    let detected_currency = detect_currency_from_html(&doc).unwrap_or_else(|| currency.to_string());

    let mut products = Vec::new();
    let card_sel = Selector::parse("div.product-cell-container").ok();
    let link_sel = Selector::parse("a.absolute-link.product-link, a.product-link").ok();

    if let (Some(card_sel), Some(link_sel)) = (card_sel, link_sel) {
        let cards: Vec<_> = doc.select(&card_sel).collect();
        tracing::debug!("Found {} product-cell-container cards", cards.len());
        products.extend(
            cards.iter().filter_map(|card| {
                parse_product_card(card, &link_sel, &detected_currency, base_url)
            }),
        );
    }

    if !products.is_empty() {
        tracing::info!("Extracted {} products from search DOM", products.len());
    } else {
        tracing::warn!("No products extracted from search DOM");
    }

    Ok(SearchResult {
        query: query.to_string(),
        total_results,
        products,
    })
}

fn parse_product_card(
    card_el: &scraper::ElementRef,
    link_sel: &Selector,
    currency: &str,
    base_url: &str,
) -> Option<ProductSummary> {
    let link = card_el.select(link_sel).next();
    let link_attrs = link.as_ref().map(|l| l.value());

    let product_id = link_attrs
        .and_then(|a| {
            a.attr("data-product-id")
                .or_else(|| a.attr("data-ga-product-id"))
        })
        .map(|s| s.to_string())
        .or_else(|| {
            link_attrs
                .and_then(|a| a.attr("href"))
                .and_then(extract_id_from_url)
        })?;

    let name = extract_card_attr(card_el, "div.product-title", "content")
        .or_else(|| extract_element_text(card_el, "div.product-title bdi, div.product-title"))
        .or_else(|| {
            link_attrs
                .and_then(|a| a.attr("title"))
                .map(|s| s.to_string())
        })?;

    let brand = link_attrs
        .and_then(|a| a.attr("data-ga-brand-name"))
        .unwrap_or("")
        .to_string();

    let price = extract_card_attr(card_el, "meta[itemprop='price']", "content")
        .and_then(|s| parse_price_str(&s))
        .or_else(|| {
            link_attrs
                .and_then(|a| a.attr("data-ga-discount-price"))
                .and_then(parse_price_str)
        })
        .unwrap_or(0.0);

    let original_price = extract_element_text(card_el, "span.price-olp bdi, span.price-olp")
        .and_then(|s| parse_price_str(&s))
        .filter(|&p| p > price);

    let rating = extract_card_rating(card_el);

    let review_count =
        extract_element_text(card_el, "a.rating-count span").and_then(|s| parse_review_count(&s));

    let in_stock = extract_card_stock_status(card_el, link_attrs);

    let product_url = link_attrs
        .and_then(|a| a.attr("href"))
        .map(|u| {
            if u.starts_with("http") {
                u.to_string()
            } else {
                format!("{}{}", base_url, u)
            }
        })
        .unwrap_or_else(|| format!("{}/pr/p/{}", base_url, product_id));

    Some(ProductSummary {
        name,
        brand,
        price,
        original_price,
        currency: currency.to_string(),
        rating,
        review_count,
        product_url,
        product_id,
        in_stock,
    })
}

fn extract_card_rating(card_el: &scraper::ElementRef) -> Option<f64> {
    let sel = Selector::parse("a.stars").ok()?;
    let el = card_el.select(&sel).next()?;
    let title = el.value().attr("title")?;
    title.split('/').next()?.trim().parse::<f64>().ok()
}

fn extract_card_stock_status(
    card_el: &scraper::ElementRef,
    link_attrs: Option<&scraper::node::Element>,
) -> bool {
    Selector::parse("div.product.ga-product, div.product")
        .ok()
        .and_then(|sel| card_el.select(&sel).next())
        .and_then(|el| el.value().attr("data-is-out-of-stock"))
        .map(|s| s.to_lowercase() != "true")
        .or_else(|| {
            link_attrs
                .and_then(|a| a.attr("data-ga-is-out-of-stock"))
                .map(|s| s.to_lowercase() != "true")
        })
        .unwrap_or(true)
}

/// Calculate how many pages needed for the desired limit.
pub fn pages_needed(limit: usize) -> usize {
    limit.div_ceil(RESULTS_PER_PAGE)
}

fn extract_card_attr(el: &scraper::ElementRef, selector: &str, attr: &str) -> Option<String> {
    let sel = Selector::parse(selector).ok()?;
    let child = el.select(&sel).next()?;
    child
        .value()
        .attr(attr)
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

fn extract_id_from_url(url: &str) -> Option<String> {
    url.split('/')
        .rev()
        .find(|segment| segment.chars().all(|c| c.is_ascii_digit()) && !segment.is_empty())
        .map(|s| s.to_string())
}

fn extract_total_results(doc: &Html) -> Option<u32> {
    // Best source: hidden span#product-count with data-count attribute
    if let Ok(sel) = Selector::parse("span#product-count") {
        if let Some(el) = doc.select(&sel).next() {
            if let Some(count) = el.value().attr("data-count") {
                if let Ok(n) = count.replace(',', "").parse::<u32>() {
                    if n > 0 {
                        return Some(n);
                    }
                }
            }
        }
    }

    // Fallback: parse "1 - 48 of 12,008 results for" text
    let sel_strs = ["div.sub-sort-title.display-items", ".display-items"];

    for sel_str in &sel_strs {
        if let Ok(sel) = Selector::parse(sel_str) {
            if let Some(el) = doc.select(&sel).next() {
                let text: String = el.text().collect();
                if let Some(idx) = text.find("of ") {
                    let after = &text[idx + 3..];
                    let num: String = after
                        .chars()
                        .take_while(|c| c.is_ascii_digit() || *c == ',')
                        .collect::<String>()
                        .replace(',', "");
                    if let Ok(n) = num.parse::<u32>() {
                        if n > 0 {
                            return Some(n);
                        }
                    }
                }
            }
        }
    }

    None
}
