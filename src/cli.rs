use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "iherb-cli",
    version,
    about = "Query iHerb product data from the command line"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Country subdomain to use (e.g., us, ch, de). Note: iHerb may override based on your IP
    #[arg(long, global = true)]
    pub country: Option<String>,

    /// Fallback currency label when auto-detection fails (e.g., USD, CHF, EUR)
    #[arg(long, global = true)]
    pub currency: Option<String>,

    /// Bypass the local cache and fetch fresh data
    #[arg(long, global = true)]
    pub no_cache: bool,

    /// Delay between requests in milliseconds (default: 2000)
    #[arg(long, global = true)]
    pub delay: Option<u64>,

    /// Run browser in headed mode for troubleshooting
    #[arg(long, global = true)]
    pub debug: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Search for products on iHerb
    Search {
        /// Search term (e.g., "vitamin c", "omega 3")
        query: String,

        /// Max number of results to return (default: 20)
        #[arg(long, default_value = "20")]
        limit: usize,

        /// Sort order: relevance, featured, best-selling, rating, most-rated,
        /// price-asc, price-desc, newest, highest-discount
        #[arg(long, value_enum, default_value_t = SortOrder::Relevance)]
        sort: SortOrder,

        /// Filter by category (e.g., supplements, vitamins, protein)
        #[arg(long)]
        category: Option<String>,
    },

    /// Get detailed product information
    Product {
        /// Numeric product ID or full iHerb product URL
        id_or_url: String,

        /// Only show a specific section: overview, description, ingredients, nutrition, suggested-use, warnings, reviews
        #[arg(long, value_enum)]
        section: Option<Section>,
    },
}

/// The orderings iHerb's own sort dropdown offers, each named after what it
/// does rather than after the `sr` number that produces it.
///
/// The dropdown on a captured results page is the source of truth for the
/// numbers; `tests/parsers/search.rs` reads it and checks every variant here
/// against it, so a variant that stops matching the site is a test failure
/// rather than a silently wrong ordering.
///
/// Two of the site's eleven orderings are deliberately absent: Heaviest (6) and
/// Lightest (7), which #3 did not propose exposing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SortOrder {
    /// iHerb's relevance ranking for the query, `sr=0`.
    ///
    /// This is **not** the same as sending no `sr` at all: an absent `sr` gets
    /// [`SortOrder::Featured`], the merchandised order. Emitting nothing here
    /// is what #3 was filed about — an agent told relevance is the neutral
    /// ranking was handed a promoted one.
    Relevance,
    /// iHerb's merchandised order, `sr=13`, and the site's own default.
    Featured,
    #[value(name = "price-asc")]
    PriceAsc,
    #[value(name = "price-desc")]
    PriceDesc,
    /// Highest average rating first. Surfaces 5.0/5 products with three
    /// reviews; [`SortOrder::MostRated`] is what "well established" wants.
    Rating,
    #[value(name = "best-selling")]
    BestSelling,
    #[value(name = "most-rated")]
    MostRated,
    Newest,
    #[value(name = "highest-discount")]
    HighestDiscount,
}

impl SortOrder {
    /// Every variant, so a sweep over the sorts cannot fall behind the enum.
    ///
    /// Written out rather than derived because `ValueEnum::value_variants` is
    /// about clap's parsing surface; this list is what the tests sweep, and it
    /// is short enough that the compiler catching a missing arm in
    /// [`SortOrder::as_url_param`] is the real guard.
    pub const ALL: &'static [SortOrder] = &[
        SortOrder::Relevance,
        SortOrder::Featured,
        SortOrder::PriceAsc,
        SortOrder::PriceDesc,
        SortOrder::Rating,
        SortOrder::BestSelling,
        SortOrder::MostRated,
        SortOrder::Newest,
        SortOrder::HighestDiscount,
    ];

    /// The `sr` value iHerb's dropdown uses for this ordering.
    pub fn sr(self) -> u8 {
        match self {
            SortOrder::Relevance => 0,
            SortOrder::Rating => 1,
            SortOrder::BestSelling => 2,
            SortOrder::PriceDesc => 3,
            SortOrder::PriceAsc => 4,
            SortOrder::Newest => 10,
            SortOrder::MostRated => 12,
            SortOrder::Featured => 13,
            SortOrder::HighestDiscount => 14,
        }
    }

    /// The query-string fragment to append, `&sr=<n>`.
    ///
    /// Every variant emits one, [`SortOrder::Featured`] included. Featured is
    /// also what an absent `sr` gets you, so it could be spelled as the empty
    /// string — but then one variant would be reachable only by omission, and
    /// "relevance emits nothing" is precisely the confusion #3 is about. Asking
    /// for an ordering by name always says which one.
    pub fn as_url_param(self) -> String {
        format!("&sr={}", self.sr())
    }

    /// This ordering's identity in a cache file name.
    ///
    /// Kept as words rather than the `sr` number so a cache directory stays
    /// readable, and stable per variant so entries survive across runs.
    pub fn as_cache_key(self) -> &'static str {
        match self {
            // Not "relevance". #3 repointed this variant from Featured (no
            // `sr`) to `sr=0`, so entries written under the old name hold
            // Featured-ordered results for a request that now means something
            // else. Changing the identifier abandons them instead of serving
            // them as if the ordering had not changed.
            SortOrder::Relevance => "relevance-sr0",
            SortOrder::Featured => "featured",
            SortOrder::PriceAsc => "price-asc",
            SortOrder::PriceDesc => "price-desc",
            SortOrder::Rating => "rating",
            SortOrder::BestSelling => "best-selling",
            SortOrder::MostRated => "most-rated",
            SortOrder::Newest => "newest",
            SortOrder::HighestDiscount => "highest-discount",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Section {
    Overview,
    Description,
    Nutrition,
    Ingredients,
    #[value(name = "suggested-use")]
    SuggestedUse,
    Warnings,
    Reviews,
}

impl Section {
    pub const ALL: &[Section] = &[
        Section::Overview,
        Section::Description,
        Section::Nutrition,
        Section::Ingredients,
        Section::SuggestedUse,
        Section::Warnings,
        Section::Reviews,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Section::Overview => "overview",
            Section::Description => "description",
            Section::Nutrition => "nutrition",
            Section::Ingredients => "ingredients",
            Section::SuggestedUse => "suggested use",
            Section::Warnings => "warnings",
            Section::Reviews => "review",
        }
    }
}
