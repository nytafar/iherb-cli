use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "iherb-cli",
    version,
    about = "Query iHerb product data from the command line",
    // Stated rather than derived. Without it clap takes the long help from the
    // doc comment of the flattened `GlobalArgs`, and `--help` opens with a
    // maintainers' note about a refactor instead of a sentence about the tool
    // (#58).
    long_about = "Query iHerb product data from the command line.\n\n\
                  Fetches a product page or a search results page through a \
                  headless Chrome, parses it, and prints Markdown for a human \
                  or `--json` for a program. Pages are cached on disk; see \
                  `iherb-cli cache`."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[command(flatten)]
    pub global: GlobalArgs,
}

/// The flags that apply to every subcommand.
///
/// Flattened out of [`Cli`] rather than listed in it, because
/// [`crate::config::AppConfig::load`] wants all of them and used to take them
/// as five positional arguments — a signature nobody could read and that #22
/// would have grown to eight.
#[derive(clap::Args, Debug)]
pub struct GlobalArgs {
    /// Country subdomain to use, e.g. us, ch, de.
    ///
    /// On its own this only picks the subdomain, and iHerb may still override
    /// it from your IP — measured: from a Norwegian address,
    /// `www.iherb.com` serves the Norwegian storefront. Pass `--currency` as
    /// well to state the storefront as a preference iHerb honours (#5).
    #[arg(long, global = true, verbatim_doc_comment)]
    pub country: Option<String>,

    /// Ask the storefront to price in this currency, e.g. USD, CHF, EUR.
    ///
    /// This asks iHerb for the currency, using the same storefront preference
    /// its own header picker uses, and then checks what came back: prices are
    /// the storefront's own in that currency, and a storefront that prices in
    /// something else is an error rather than a relabelling. Nothing is ever
    /// converted, and no price is captioned with a currency the page did not
    /// publish.
    ///
    /// It also pins the storefront against iHerb's IP geolocation, which is
    /// otherwise free to override `--country`. Omit it to take whatever iHerb
    /// serves.
    #[arg(long, global = true, verbatim_doc_comment)]
    pub currency: Option<String>,

    /// Touch the cache for neither reads nor writes.
    ///
    /// **This changed meaning in #22 and there is no alias.** It used to
    /// disable reads only and write the result anyway, so a caller asking not
    /// to touch the cache still got files written. That behaviour is what
    /// `--refresh` is now called; this flag now does what its name says.
    ///
    /// Nothing has been released, so the break costs no consumer anything
    /// (#38). If you want the old behaviour, pass `--refresh`.
    ///
    /// Given together with `--refresh`, this one wins: it is the stronger of
    /// the two requests.
    #[arg(long, global = true, verbatim_doc_comment)]
    pub no_cache: bool,

    /// Skip the cache on the way in, write the result on the way out.
    ///
    /// What you want to re-read a page whose price may have moved and keep the
    /// new answer for next time. This is what `--no-cache` used to do.
    #[arg(long, global = true, verbatim_doc_comment)]
    pub refresh: bool,

    /// How long a cache entry stays usable, e.g. `30d`, `12h`, `45m`, `90s`.
    ///
    /// Default `30d`. That is far too long for a price and about right for a
    /// supplement facts panel, and both live in one cached record — so a
    /// per-data-kind TTL is a change to the *model*, not to this flag, and
    /// belongs with #15 and DECISION-01. One number for the whole record is
    /// what this tool can honestly offer today.
    #[arg(long, global = true, value_name = "DURATION", verbatim_doc_comment)]
    pub cache_ttl: Option<String>,

    /// Path to the Chrome or Chromium executable.
    ///
    /// Highest priority in the chain: this flag, then `IHERB_BROWSER_PATH`,
    /// then `browser_path` in the config file. Without any of them the tool
    /// downloads Chrome for Testing on first use — which is a slow surprise for
    /// an agent that cannot set an environment variable on a subprocess and had
    /// no flag to reach for (#22).
    ///
    /// A path you name here binds: if it does not exist the run fails with
    /// `invalid_input` (2) naming the path, rather than quietly using some
    /// other browser (#55). The same is true of the environment variable and
    /// the config file. Only the automatic search — system Chrome, then a
    /// previously downloaded Chrome, then the download — falls through, because
    /// nobody named those.
    #[arg(long, global = true, value_name = "PATH", verbatim_doc_comment)]
    pub browser_path: Option<std::path::PathBuf>,

    /// Read this config file instead of the one under the user's config dir.
    ///
    /// A path given here must exist and must parse; a missing or malformed file
    /// is an error rather than a silent fallback to the defaults, because you
    /// asked for that file by name. The default path stays optional — most runs
    /// have no config file at all.
    #[arg(long, global = true, value_name = "PATH", verbatim_doc_comment)]
    pub config: Option<std::path::PathBuf>,

    /// Delay between requests in milliseconds (default: 500)
    ///
    /// Politeness between one request and the next, and nothing else. Until
    /// #11 it was also slept before every navigation as a guess at page-load
    /// time — 2000 ms, charged per page, about a third of a 25-product
    /// comparison's wall clock — which is now the readiness selectors' job.
    #[arg(long, global = true, verbatim_doc_comment)]
    pub delay: Option<u64>,

    /// How many times one page is fetched before the run gives up (default: 3).
    ///
    /// A total, not a count of retries on top of a first try, so `1` means
    /// "try once and report what happened". Between tries the wait doubles:
    /// 1s, then 2s, then 4s.
    ///
    /// A Cloudflare block ends the sequence whatever this says. Retrying
    /// seconds later from the same address with the same fingerprint cannot
    /// succeed, and it spends the rate limit a later run — or a hand-cleared
    /// profile from `iherb-cli setup` — will want (#23).
    #[arg(long, global = true, value_name = "N", verbatim_doc_comment)]
    pub attempts: Option<u32>,

    /// How many times one page is checked for a Cloudflare interstitial
    /// (default: 3).
    ///
    /// Each look after the first waits up to 12 seconds for the challenge to
    /// clear itself, re-checking every second, so the default is worth up to
    /// 24 seconds per page. Raise it on a storefront that challenges often and
    /// you are willing to wait out; drop it to `1` to fail fast and let a
    /// caller decide.
    ///
    /// This is spent *inside* one navigation attempt, so the worst case is
    /// this many looks per `--attempts` — except that a block does not lead to
    /// another attempt (#23).
    #[arg(long, global = true, value_name = "N", verbatim_doc_comment)]
    pub cloudflare_attempts: Option<u32>,

    /// Print how long each phase of every navigation took, on stderr.
    ///
    /// One `key=value` line per page: `goto_ms`, `cloudflare_check_ms`,
    /// `wait_selector_ms`, `html_extract_ms`, the total, and which readiness
    /// selector matched. This is what makes a slow run diagnosable rather than
    /// merely slow — a long Cloudflare check and a long selector wait call for
    /// opposite responses.
    ///
    /// Independent of `--debug`, which turns every log line up and writes an
    /// HTML dump per page.
    #[arg(long, global = true, verbatim_doc_comment)]
    pub timing: bool,

    /// Keep the browser profile in this directory instead of the default one.
    ///
    /// A profile is where Chrome keeps cookies, storefront preferences and
    /// whatever Cloudflare clearance a run earned, so reusing one is what lets
    /// clearance survive between runs (#12). Run `iherb-cli setup` against a
    /// directory once to clear a challenge by hand; every later run pointed at
    /// it starts from that state.
    ///
    /// **A directory you name here is never deleted by this tool**, and it
    /// binds: if another run already holds it the run fails rather than
    /// quietly using somewhere else. The default profile, under the data
    /// directory, degrades to a throwaway one with a warning instead — nobody
    /// named it, so there is no constraint to break.
    #[arg(long, global = true, value_name = "PATH", verbatim_doc_comment)]
    pub profile_dir: Option<std::path::PathBuf>,

    /// Use a throwaway profile that is deleted when the run ends.
    ///
    /// What every run did before #12. Each run then starts cold and
    /// cookie-less, which is the fingerprint Cloudflare scores worst, and any
    /// clearance it earns dies with it. Useful for a run that must leave
    /// nothing behind, and for two runs that would otherwise contend for one
    /// profile directory.
    #[arg(long, global = true, verbatim_doc_comment)]
    pub no_profile: bool,

    /// Emit one JSON document on stdout instead of Markdown.
    ///
    /// Success or failure, exactly one document and nothing else: logging goes
    /// to stderr, and a failure is an envelope with `ok: false`, a stable
    /// `error_type` and a stable exit code (#9). Every document carries a
    /// `schema_version` and a `meta` block naming the storefront, currency and
    /// both timestamps, so a stored record is still interpretable months later
    /// (#44). `iherb-cli --help` documents the exit codes; the README carries
    /// the schema history.
    #[arg(long, global = true, verbatim_doc_comment)]
    pub json: bool,

    /// Log at debug level and dump every fetched page to disk.
    ///
    /// Headless, and that is the point (#62). The dump is the cheapest way to
    /// see what iHerb actually served, and it used to require a visible
    /// browser window, which made it unavailable in CI, over SSH and in an
    /// unattended run — exactly where "the scraper broke and I cannot see the
    /// page" gets asked. Dumps land in the `dumps` subdirectory of the cache
    /// directory `cache path` prints. For a window to watch, add `--headful`.
    #[arg(long, global = true, verbatim_doc_comment)]
    pub debug: bool,

    /// Show a real browser window. Implies nothing else.
    ///
    /// The window is the half of `--debug` that used to be missing (#47) and
    /// is now a flag of its own (#62): it turns no logging up and writes no
    /// dump, and `--debug --headful` is the old combined behaviour. This is
    /// the flag for watching a page load, or for completing a Cloudflare
    /// challenge by hand.
    #[arg(long, global = true, verbatim_doc_comment)]
    pub headful: bool,
}

impl GlobalArgs {
    /// The globals as a command line that passed none of them.
    ///
    /// Not `Default`, because "default" is what the *configuration* resolves to
    /// — country `us`, a 30-day TTL — and this is the absence of a flag, which
    /// is a different thing: `country: None` means the environment and the
    /// config file still get a say. Handy for tests and for a library caller
    /// that has no `argv`.
    pub fn none() -> Self {
        Self {
            country: None,
            currency: None,
            no_cache: false,
            refresh: false,
            cache_ttl: None,
            browser_path: None,
            config: None,
            delay: None,
            attempts: None,
            cloudflare_attempts: None,
            timing: false,
            profile_dir: None,
            no_profile: false,
            json: false,
            debug: false,
            headful: false,
        }
    }
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

    /// Inspect and manage the local cache
    Cache {
        #[command(subcommand)]
        action: CacheCommand,
    },

    /// Open a browser window on the storefront and wait, so a profile can be
    /// prepared by hand
    ///
    /// Chrome opens on the storefront `--country` selects and stays there until
    /// you close the window. Whatever you do in it — clear a Cloudflare
    /// challenge, pick a country and currency, sign in — is written to the
    /// profile directory, and every later run pointed at that directory starts
    /// from it (#12).
    ///
    /// The window is not optional here: a headless `setup` would be a command
    /// that asks a human to do something they cannot see. `--headful` is
    /// implied, and `--no-profile` is refused, because a profile deleted on the
    /// way out is nothing to set up.
    #[command(verbatim_doc_comment)]
    Setup,
}

/// What `cache` can be asked to do.
///
/// File operations over the existing cache directory, and nothing more. #27
/// considers a SQLite-backed cache and is parked; none of this is a step
/// towards one.
#[derive(Subcommand, Debug)]
pub enum CacheCommand {
    /// Print the resolved cache directory
    Path,

    /// Count the entries, their bytes, and the oldest and newest
    Stats,

    /// Remove cache entries
    ///
    /// **This deletes files.** It only ever removes regular `.json` files
    /// sitting directly in the resolved cache directory: never a symlink, never
    /// anything in a subdirectory, and never anything outside it. Whatever it
    /// removed is reported.
    ///
    /// With no filter it removes everything, and that needs `--all` said out
    /// loud — an agent that types `cache clear` by accident should not silently
    /// lose the cache.
    #[command(verbatim_doc_comment)]
    Clear {
        /// Only entries older than this, e.g. `7d`, `12h`, `45m`
        #[arg(long, value_name = "DURATION")]
        older_than: Option<String>,

        /// Only entries for this storefront, e.g. `no`
        ///
        /// Product entries name their country in the file name and are matched
        /// exactly. **Search entries cannot be matched**: their name is a hash
        /// of the whole request, so the country is inside it and not readable
        /// off the file. They are left alone and counted, so the report says
        /// how many could not be attributed rather than implying the storefront
        /// is now clear.
        #[arg(long, value_name = "COUNTRY", verbatim_doc_comment)]
        country: Option<String>,

        /// Required to remove everything, when no other filter is given
        #[arg(long)]
        all: bool,
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
