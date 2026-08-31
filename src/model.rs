use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductSummary {
    pub name: String,
    pub brand: String,
    pub price: f64,
    pub original_price: Option<f64>,
    pub currency: String,
    pub rating: Option<f64>,
    pub review_count: Option<u32>,
    pub product_url: String,
    pub product_id: String,
    pub in_stock: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductDetail {
    pub name: String,
    pub brand: String,
    pub price: f64,
    pub original_price: Option<f64>,
    pub currency: String,
    pub rating: Option<f64>,
    pub review_count: Option<u32>,
    pub product_url: String,
    pub product_id: String,
    /// Whether the product can be bought, or `None` when no signal on the page
    /// said either way.
    ///
    /// This is an `Option` rather than a `bool` on purpose (#30, #31, #28).
    /// Every parser used to default it to `true`, so a product that was out of
    /// stock — or a page whose stock markup we no longer understand — was
    /// reported as purchasable. "We could not tell" is a different answer from
    /// "yes", and a caller deciding whether to buy something needs them apart.
    pub in_stock: Option<bool>,
    pub description: Option<String>,
    pub product_code: Option<String>,
    pub upc: Option<String>,
    pub ingredients: Option<String>,
    pub supplement_facts: Option<SupplementFacts>,
    pub suggested_use: Option<String>,
    pub warnings: Option<String>,
    pub shipping_weight: Option<String>,
    pub category_breadcrumb: Option<Vec<String>>,
    pub review_distribution: Option<ReviewDistribution>,
    /// Where every field above came from, and whether the record looks healthy.
    ///
    /// `#[serde(default)]` so a cache file written before provenance existed
    /// still deserializes; it comes back [`Strategy::Unrecorded`] with no
    /// sources, which is the truth about such a file.
    #[serde(default)]
    pub extraction: Extraction,
}

/// Where a field's value came from.
///
/// The point of this type is that [`Source::Absent`] is a value. Before it
/// existed, "iHerb publishes no product code for this item" and "our selector
/// rotted and ate the product code" were both `Option::None`, and no caller and
/// no test could tell them apart. That conflation is the mechanism behind most
/// of the bugs filed in this round (#28).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The `application/ld+json` `Product` block.
    JsonLd,
    /// `window.IHR_DL.product` / `window.PRODUCT_DETAILS`, read by evaluating
    /// JS on the live page.
    JsGlobals,
    /// CSS selectors over the page HTML, whether as the last-resort strategy or
    /// as enrichment on top of another one.
    Dom,
    /// The field has a value, and **nobody read it off the page**: it is a
    /// hardcoded constant or a label the caller passed in.
    ///
    /// Distinct from [`Source::Absent`] on purpose. Absent means there is no
    /// value; this means there is one and it should not be trusted. Recording a
    /// defaulted value as `Absent` would create a second conflation — absent
    /// versus fabricated — which is the exact class of bug this type exists to
    /// kill, and recording it as `JsonLd` would have provenance vouch for a
    /// value JSON-LD never carried.
    ///
    /// `currency` is the live case (#5): every path falls back to `"USD"` or to
    /// the `--currency` label when the page publishes no currency marker.
    Defaulted,
    /// Every strategy ran and none produced this field.
    Absent,
}

impl Source {
    /// Whether this source means a strategy actually read the value off the
    /// page. [`Source::Defaulted`] and [`Source::Absent`] do not.
    pub fn is_attested(self) -> bool {
        matches!(self, Source::JsonLd | Source::JsGlobals | Source::Dom)
    }
}

/// Which strategy produced the base record, before DOM enrichment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    JsonLd,
    JsGlobals,
    Dom,
    /// Not produced by an extractor at all: a hand-built record, or one read
    /// back from a cache file written before provenance existed.
    Unrecorded,
}

/// The provenance a [`ProductDetail`] carries: which strategy produced it,
/// whether it was enriched, and where each field came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extraction {
    pub strategy: Strategy,
    /// Whether [`crate::scraper::product::enrich_from_html`] ran. It runs on
    /// every path, so `false` on a freshly extracted record means something
    /// went wrong.
    pub enriched: bool,
    /// Field name -> where its value came from. A field missing from this map
    /// is [`Source::Absent`]; read it through [`Extraction::source_of`] rather
    /// than indexing, so the two cannot drift apart.
    sources: BTreeMap<String, Source>,
}

impl Default for Extraction {
    fn default() -> Self {
        Self {
            strategy: Strategy::Unrecorded,
            enriched: false,
            sources: BTreeMap::new(),
        }
    }
}

impl Extraction {
    pub fn new(strategy: Strategy) -> Self {
        Self {
            strategy,
            ..Self::default()
        }
    }

    /// Where `field` came from. A field nothing claimed is [`Source::Absent`].
    pub fn source_of(&self, field: &str) -> Source {
        self.sources.get(field).copied().unwrap_or(Source::Absent)
    }

    /// Attribute `field` to `source`, unless something already claimed it.
    ///
    /// First writer wins, because the strategies run in order of trust: if
    /// JSON-LD supplied the price, DOM enrichment filling a gap must not
    /// relabel it. Use [`Extraction::reclaim`] where a later pass genuinely
    /// replaces a value rather than filling a gap.
    pub fn claim(&mut self, field: &str, source: Source) {
        self.sources.entry(field.to_string()).or_insert(source);
    }

    /// Attribute `field` to `source`, overwriting any earlier claim. For the
    /// one case where a later pass really does replace an earlier value.
    pub fn reclaim(&mut self, field: &str, source: Source) {
        self.sources.insert(field.to_string(), source);
    }
}

/// A scrape's report on itself: what produced it and whether to trust it.
///
/// This is the shape #9 renders under `--json`. It is derived from the record
/// rather than stored on it, so it cannot go stale.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionHealth {
    pub strategy: Strategy,
    pub enriched: bool,
    /// Every tracked field and where it came from, `Absent` included.
    pub sources: BTreeMap<String, Source>,
    /// The tracked fields no strategy produced, in declaration order.
    pub fields_absent: Vec<String>,
    /// The tracked fields that have a value nobody read off the page, in
    /// declaration order. See [`Source::Defaulted`].
    ///
    /// Separate from `fields_absent` because they are different problems: a
    /// caller can ignore an absent field, but a defaulted one will silently
    /// look like data.
    pub fields_defaulted: Vec<String>,
    /// True when a field [`ProductDetail::EXPECTED_FIELDS`] says every product
    /// page publishes was not actually read off the page — absent, or present
    /// only as a default.
    ///
    /// This is the "our selectors rotted" signal, and it is deliberately
    /// distinct from "this product has no supplement facts because it is a
    /// hairbrush" — which is why the expected set is short.
    pub degraded: bool,
}

impl ProductDetail {
    /// Fields any iHerb product page publishes.
    ///
    /// One of these coming back [`Source::Absent`] means extraction is broken,
    /// not that the product lacks the attribute. Kept deliberately short:
    /// `rating` and `review_count` are out because a product with no reviews
    /// legitimately has neither, and supplement facts are out because plenty of
    /// products are not supplements.
    pub const EXPECTED_FIELDS: &'static [&'static str] = &[
        "name",
        "brand",
        "price",
        "currency",
        "in_stock",
        "product_code",
        "upc",
    ];

    /// Every field provenance tracks, paired with whether this record has a
    /// value for it.
    ///
    /// Name and presence are declared together on purpose: one list, so the
    /// registry of tracked fields cannot drift out of step with the test for
    /// whether each one is filled.
    ///
    /// # This destructuring is load-bearing
    ///
    /// `self` is taken apart field by field rather than read through `self.x`,
    /// and the pattern deliberately has **no `..` rest**. That makes the
    /// compiler enforce the registry: add a field to [`ProductDetail`] and this
    /// stops compiling with "pattern does not mention field", so the only way to
    /// add a field is to decide, then and there, whether provenance tracks it.
    /// Before this, a new field silently had no provenance and no source — the
    /// failure mode the whole type exists to prevent, reintroduced one field at
    /// a time.
    ///
    /// If you are here because the build broke: add your field to the list
    /// below, or bind it as `your_field: _` with a line saying why it is not
    /// extracted data. Do not add `..`.
    pub fn field_presence(&self) -> Vec<(&'static str, bool)> {
        let Self {
            name,
            brand,
            price,
            original_price,
            currency,
            rating,
            review_count,
            product_url,
            // Not tracked: the caller's input, echoed back. No strategy
            // produces it and it is never absent, so a provenance entry for it
            // would only ever say the same thing.
            product_id: _,
            in_stock,
            description,
            product_code,
            upc,
            ingredients,
            supplement_facts,
            suggested_use,
            warnings,
            shipping_weight,
            category_breadcrumb,
            review_distribution,
            // Not tracked: the provenance record itself.
            extraction: _,
        } = self;

        vec![
            ("name", !name.is_empty()),
            ("brand", !brand.is_empty()),
            ("price", *price > 0.0),
            ("original_price", original_price.is_some()),
            ("currency", !currency.is_empty()),
            ("rating", rating.is_some()),
            ("review_count", review_count.is_some()),
            ("product_url", !product_url.is_empty()),
            ("in_stock", in_stock.is_some()),
            ("description", description.is_some()),
            ("product_code", product_code.is_some()),
            ("upc", upc.is_some()),
            ("ingredients", ingredients.is_some()),
            ("supplement_facts", supplement_facts.is_some()),
            ("suggested_use", suggested_use.is_some()),
            ("warnings", warnings.is_some()),
            ("shipping_weight", shipping_weight.is_some()),
            ("category_breadcrumb", category_breadcrumb.is_some()),
            ("review_distribution", review_distribution.is_some()),
        ]
    }

    /// Where `field` came from. Shorthand for `self.extraction.source_of`.
    pub fn source_of(&self, field: &str) -> Source {
        self.extraction.source_of(field)
    }

    /// Attribute every value this record holds, and that nothing has claimed
    /// yet, to `source`.
    ///
    /// Derived from the values rather than written out by hand at each parser,
    /// so a parser that starts filling a new field cannot forget to record it.
    pub fn claim_unattributed(&mut self, source: Source) {
        for (field, present) in self.field_presence() {
            if present {
                self.extraction.claim(field, source);
            }
        }
    }

    /// This record's report on itself.
    pub fn health(&self) -> ExtractionHealth {
        let mut sources = BTreeMap::new();
        let mut fields_absent = Vec::new();
        let mut fields_defaulted = Vec::new();

        for (field, _) in self.field_presence() {
            let source = self.extraction.source_of(field);
            match source {
                Source::Absent => fields_absent.push(field.to_string()),
                Source::Defaulted => fields_defaulted.push(field.to_string()),
                _ => {}
            }
            sources.insert(field.to_string(), source);
        }

        // Not `== Absent`: a field that carries a hardcoded constant was not
        // produced either, and treating it as if it had been makes its slot in
        // EXPECTED_FIELDS a rot-detector that can never fire.
        let degraded = Self::EXPECTED_FIELDS
            .iter()
            .any(|f| !self.extraction.source_of(f).is_attested());

        ExtractionHealth {
            strategy: self.extraction.strategy,
            enriched: self.extraction.enriched,
            sources,
            fields_absent,
            fields_defaulted,
            degraded,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupplementFacts {
    pub serving_size: Option<String>,
    pub servings_per_container: Option<String>,
    pub nutrients: Vec<Nutrient>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nutrient {
    pub name: String,
    pub amount: String,
    pub daily_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewDistribution {
    pub five_star: Option<f64>,
    pub four_star: Option<f64>,
    pub three_star: Option<f64>,
    pub two_star: Option<f64>,
    pub one_star: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub query: String,
    pub total_results: Option<u32>,
    pub products: Vec<ProductSummary>,
    /// What the walk that produced `products` actually did.
    ///
    /// `#[serde(default)]` so a cache entry written before this existed still
    /// deserializes; it comes back saying nothing, which is the truth about
    /// such a file, and a request it cannot satisfy is refetched rather than
    /// assumed to be complete.
    #[serde(default)]
    pub fetch: SearchFetch,
}

/// What a search walk did, recorded on its result (#6).
///
/// A search entry holds however many products the run that wrote it happened to
/// fetch, and the cache key cannot say how many that was — two runs differing
/// only in `--limit` share one entry on purpose, because the entry holds
/// everything either of them fetched. So a later, wider run reading that entry
/// was handed the narrow run's results and had no way to tell it had been
/// short-changed. These two fields are what it reads to tell.
///
/// Both are `Option` because "this record does not say" is a real state: an
/// entry written before this existed. Zero pages and "not exhausted" would be
/// claims about a fetch nobody watched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFetch {
    /// Result pages walked.
    pub pages_fetched: Option<usize>,
    /// Whether the walk stopped because iHerb ran out of results, rather than
    /// because it had gathered what it was asked for or hit its page budget.
    ///
    /// `Some(true)` is what makes a short record complete: 45 products is every
    /// product there is, not the first page of thousands. Anything else leaves
    /// a short record short, which is the case #6 is about.
    pub exhausted: Option<bool>,
}
