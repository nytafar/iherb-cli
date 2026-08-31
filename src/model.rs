use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One product as a search results page presents it.
///
/// The search half of #28's provenance (#49). `ProductDetail` stopped
/// fabricating values in #30/#31 and started recording where each one came
/// from; the search path did not, and carried the same class of confidently
/// wrong data — a currency nobody read, a price of `0.0` for a card whose price
/// would not parse, and an `in_stock` that said "yes" when the card said
/// nothing. It now answers the same questions in the same shape, so a caller —
/// and #9's `--json` — sees one provenance block rather than one that exists on
/// products and silently not on search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductSummary {
    pub name: String,
    pub brand: String,
    /// The price on the card, or `None` when neither the microdata nor the
    /// card's own attributes carried one that parsed.
    ///
    /// An `Option` rather than an `f64` because the fallback was `0.0`, which a
    /// caller cannot tell from a genuinely free product — the same conflation
    /// `in_stock` had before #31. (`ProductDetail::price` still uses `0.0` that
    /// way; that is #50, and not this.)
    pub price: Option<f64>,
    pub original_price: Option<f64>,
    /// The currency the results page published, or `None` when it published
    /// none.
    ///
    /// An `Option` for the same reason `price` is one (#5). The card used to
    /// carry `detect_currency_from_html(..).unwrap_or(--currency)`, so a page
    /// with no currency marker got the label the caller happened to pass, and
    /// `9.60` from the US storefront was printed as `CHF 9.60`. #49 stopped
    /// provenance vouching for that value; it did not stop the value existing.
    /// A mislabelled price is worse than a missing one — an agent comparing
    /// `CHF 9.60` here against `CHF 12.00` from a real Swiss storefront makes a
    /// confidently wrong recommendation — so there is now no label to
    /// substitute, and an unread currency is `None`.
    pub currency: Option<String>,
    pub rating: Option<f64>,
    pub review_count: Option<u32>,
    pub product_url: String,
    pub product_id: String,
    /// Whether the product can be bought, or `None` when nothing on the card
    /// said either way. The same `Option` and for the same reason as
    /// [`ProductDetail::in_stock`] (#31): the card's stock attribute is absent
    /// often enough that defaulting to `true` reported unbuyable products as
    /// purchasable.
    pub in_stock: Option<bool>,
    /// Where every field above came from.
    ///
    /// `#[serde(default)]` so a cache entry written before this existed still
    /// deserializes, as [`Strategy::Unrecorded`] with no sources — which is the
    /// truth about such a file.
    #[serde(default)]
    pub extraction: Extraction,
}

impl ProductSummary {
    /// Fields any search card publishes.
    ///
    /// One of these coming back unattested means the card selectors have
    /// rotted, not that the product lacks the attribute. Short, for the same
    /// reason as [`ProductDetail::EXPECTED_FIELDS`]: `rating` and
    /// `review_count` are out because an unreviewed product legitimately has
    /// neither, `original_price` because most products are not discounted, and
    /// `in_stock` because the card genuinely omits the attribute on products
    /// that are in stock.
    pub const EXPECTED_FIELDS: &'static [&'static str] = &[
        "name",
        "brand",
        "price",
        "currency",
        "product_url",
        "product_id",
    ];

    /// Every field provenance tracks, paired with whether this record has a
    /// value for it.
    ///
    /// # This destructuring is load-bearing
    ///
    /// Same rule as [`ProductDetail::field_presence`], and for the same reason:
    /// no `..` rest, so adding a field to this struct stops the build until
    /// someone decides whether provenance tracks it. See that method for the
    /// argument.
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
            // Tracked, unlike on `ProductDetail`. There the id is the caller's
            // input echoed back; here it is read off the card, and a card whose
            // id could not be read produces no summary at all.
            product_id,
            in_stock,
            // Not tracked: the provenance record itself.
            extraction: _,
        } = self;

        vec![
            ("name", !name.is_empty()),
            ("brand", !brand.is_empty()),
            ("price", price.is_some()),
            ("original_price", original_price.is_some()),
            ("currency", currency.is_some()),
            ("rating", rating.is_some()),
            ("review_count", review_count.is_some()),
            ("product_url", !product_url.is_empty()),
            ("product_id", !product_id.is_empty()),
            ("in_stock", in_stock.is_some()),
        ]
    }

    /// Where `field` came from. Shorthand for `self.extraction.source_of`.
    pub fn source_of(&self, field: &str) -> Source {
        self.extraction.source_of(field)
    }

    /// Attribute every value this record holds, and that nothing has claimed
    /// yet, to `source`.
    pub fn claim_unattributed(&mut self, source: Source) {
        let presence = self.field_presence();
        claim_present(&mut self.extraction, &presence, source);
    }

    /// This record's report on itself, in the same shape a [`ProductDetail`]
    /// reports — which is the point of #49: #9 renders one block, not two.
    pub fn health(&self) -> ExtractionHealth {
        derive_health(
            &self.extraction,
            &self.field_presence(),
            Self::EXPECTED_FIELDS,
        )
    }
}

/// Attribute every present field that nothing has claimed yet to `source`.
///
/// Derived from the values rather than written out by hand at each parser, so a
/// parser that starts filling a new field cannot forget to record it.
fn claim_present(extraction: &mut Extraction, presence: &[(&'static str, bool)], source: Source) {
    for (field, present) in presence {
        if *present {
            extraction.claim(field, source);
        }
    }
}

/// Derive a record's report on itself from its field registry.
///
/// Shared by [`ProductDetail`] and [`ProductSummary`] so the two cannot drift
/// into reporting the same thing in two shapes — which is what #49 was about at
/// the level below this one.
fn derive_health(
    extraction: &Extraction,
    presence: &[(&'static str, bool)],
    expected: &[&'static str],
) -> ExtractionHealth {
    let mut sources = BTreeMap::new();
    let mut fields_absent = Vec::new();
    let mut fields_defaulted = Vec::new();
    let mut fields_malformed = Vec::new();

    for (field, _) in presence {
        let source = extraction.source_of(field);
        match source {
            Source::Absent => fields_absent.push((*field).to_string()),
            Source::Defaulted => fields_defaulted.push((*field).to_string()),
            Source::Malformed => fields_malformed.push((*field).to_string()),
            Source::JsonLd | Source::JsGlobals | Source::Dom => {}
        }
        sources.insert((*field).to_string(), source);
    }

    // Not `== Absent`: a field that carries a hardcoded constant was not
    // produced either, and treating it as if it had been makes its slot in the
    // expected set a rot-detector that can never fire.
    let expected_unread = expected
        .iter()
        .any(|f| !extraction.source_of(f).is_attested());

    // A malformed field degrades the record wherever it is. The expected set
    // answers "should this page have had one?", which only matters when the
    // field is missing; a malformed field was *there* and we could not read it,
    // and there is no page for which that is fine.
    let degraded = expected_unread || !fields_malformed.is_empty();

    ExtractionHealth {
        strategy: extraction.strategy,
        enriched: extraction.enriched,
        sources,
        fields_absent,
        fields_defaulted,
        fields_malformed,
        degraded,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductDetail {
    pub name: String,
    pub brand: String,
    pub price: f64,
    pub original_price: Option<f64>,
    /// The currency the product page published, or `None` when it published
    /// none.
    ///
    /// The same `Option` and for the same reason as
    /// [`ProductSummary::currency`] (#5). Every strategy used to substitute
    /// something here when the page said nothing — JSON-LD a hardcoded
    /// `"USD"`, the JS globals and the DOM fallback the `--currency` label —
    /// and the substituted value was indistinguishable from a currency iHerb
    /// actually published.
    pub currency: Option<String>,
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
    /// **The page carried this field and we could not read it.** There is no
    /// value, and that is our fault rather than the page's.
    ///
    /// The third state the `Option`-shaped world could not express. `Absent`
    /// says the page published nothing; this says it published something in a
    /// shape our selectors no longer understand — which is rot, and rot that
    /// reports itself as ordinary absence is invisible until someone happens to
    /// re-fetch the page and notice.
    ///
    /// `review_distribution` is the live case (#32). The histogram widget draws
    /// its bars with no semantic marker of which star level each stands for, so
    /// the level is inferred from how the star glyphs are drawn. A hydrated
    /// widget whose bars yield nothing, and a widget whose bars resolve to the
    /// same level twice, are both this — never [`Source::Absent`], which is
    /// what an unhydrated shell or a page with no widget earns.
    ///
    /// Deliberately **not** [`Source::Defaulted`]. Defaulted means there is a
    /// value and it should not be trusted; this means there is no value at all.
    /// Filing it as `Defaulted` would put a field with nothing in it on the
    /// "looks like data" list, which is the opposite of what a reader needs.
    Malformed,
    /// Every strategy ran and none produced this field.
    Absent,
}

impl Source {
    /// Whether this source means a strategy actually read the value off the
    /// page. [`Source::Defaulted`], [`Source::Malformed`] and
    /// [`Source::Absent`] do not.
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
    /// The tracked fields the page carried and extraction could not read, in
    /// declaration order. See [`Source::Malformed`].
    ///
    /// Separate from `fields_absent` for the same reason `fields_defaulted` is:
    /// an absent field is the page's answer, and a malformed one is ours. Any
    /// entry here means a selector has rotted, whatever the field.
    pub fields_malformed: Vec<String>,
    /// True when extraction looks broken rather than merely thin.
    ///
    /// Two ways to earn it:
    ///
    ///  1. A field [`ProductDetail::EXPECTED_FIELDS`] says every product page
    ///     publishes was not actually read off the page — absent, or present
    ///     only as a default.
    ///  2. **Any** field is [`Source::Malformed`], expected or not. A malformed
    ///     field is a page that carried data we could not read, which is rot by
    ///     definition; it does not need to be on a list of fields every page has
    ///     to prove the selectors have drifted.
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
            ("currency", currency.is_some()),
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
        let presence = self.field_presence();
        claim_present(&mut self.extraction, &presence, source);
    }

    /// This record's report on itself.
    pub fn health(&self) -> ExtractionHealth {
        derive_health(
            &self.extraction,
            &self.field_presence(),
            Self::EXPECTED_FIELDS,
        )
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
