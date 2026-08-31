use thiserror::Error;

#[derive(Error, Debug)]
pub enum IherbError {
    /// The caller's arguments cannot produce a request. Detected before any
    /// browser or network work happens.
    ///
    /// Typed rather than left as an `anyhow` string (#9) because the exit code
    /// is the whole point: an empty `--query`, a `--limit` of 0, an unknown
    /// `--category`, a product identifier that is neither an id nor a URL and
    /// an unknown `--country` were all untyped, and an unclassifiable error is
    /// `internal_error` — which tells a caller to file a bug about input it
    /// could simply correct. The unknown country code was worse still: it was
    /// [`IherbError::Navigation`], so a permanently invalid argument reported
    /// itself as a transient network failure worth retrying.
    #[error("{0}")]
    InvalidInput(String),

    #[error("Failed to launch browser: {0}")]
    BrowserLaunch(String),

    #[error("Browser navigation failed: {0}")]
    Navigation(String),

    #[error("Cloudflare challenge could not be solved after {0} attempts")]
    CloudflareBlocked(u32),

    /// The page said the product is gone: a 404, or a not-found page. Reserved
    /// for that — a caller seeing this should stop asking about the id.
    #[error("Product not found: {0}")]
    ProductNotFound(String),

    /// The page loaded, was not a 404, and no extraction strategy produced
    /// usable data from it.
    ///
    /// Distinct from [`IherbError::ProductNotFound`] on purpose (#28). This one
    /// means the scraper is broken or the site changed shape — retrying the
    /// same id is reasonable, and a human should look. Reporting it as
    /// "not found" is the worst available misclassification, because it tells
    /// the caller to give up on an id that is fine.
    #[error("Extraction failed for product {0}: the page loaded but no strategy produced usable data. The scraper may need updating.")]
    ParseFailed(String),

    /// A page that should have carried listings carried none: an empty result
    /// set, or the end of a paginated walk.
    ///
    /// Typed for the same reason [`IherbError::InvalidInput`] is (#9). An empty
    /// search is the most ordinary failure this tool has, and as an untyped
    /// `anyhow` string it classified as `internal_error` — "the scraper is
    /// broken, file a bug" — for a query that simply matches nothing.
    ///
    /// Whether an empty result should be an error *at all* is a separate and
    /// live question: a caller walking a catalog would rather have an empty
    /// list and a zero exit than a failure. That is a behaviour change and it
    /// belongs with the batch and catalog work (#10, #21). This variant only
    /// gives the behaviour that already ships an honest name.
    #[error("{0}")]
    EmptyPageOrCatalogEnd(String),

    /// `--currency` named a currency, and the storefront did not price in it.
    ///
    /// An error rather than a relabelling (#5). iHerb prices in the currency of
    /// the storefront `--country` selects; the flag cannot convert, so the only
    /// honest thing it can do about a disagreement is refuse to answer.
    #[error("Storefront currency is {actual}, not the {expected} that --currency asked for ({what}). iHerb prices in its storefront's own currency and this tool does not convert; --country selects the storefront.")]
    CurrencyMismatch {
        expected: String,
        actual: String,
        what: String,
    },

    #[error("Chrome download failed: {0}")]
    ChromeDownload(String),

    #[error("Cache error: {0}")]
    Cache(String),

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// What went wrong, as a caller has to act on it (#9).
///
/// One variant per documented `error_type`, each with a stable exit code. The
/// point of the type is that the four failures a caller must respond to
/// differently — skip this id, retry later, fix the environment, file a bug —
/// are four different numbers rather than one `1`.
///
/// The codes are grouped by what a caller does about them: `2` is the caller's
/// input, `1x` the local environment, `2x` the page, `3x` the machine, `4x` the
/// data, and [`ErrorKind::Internal`] sits outside all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The arguments cannot produce a request. Fix them and try again.
    InvalidInput,
    /// Chrome would not start. The environment needs attention.
    BrowserLaunchFailed,
    /// Chrome could not be downloaded.
    ChromeDownloadFailed,
    /// The page did not load in time. Worth retrying.
    NavigationTimeout,
    /// The page did not load, and not because of the clock.
    NavigationFailed,
    /// Cloudflare would not let us through. Retry later, from elsewhere.
    CloudflareBlocked,
    /// iHerb says the product is gone. Stop asking about this id.
    ProductNotFound,
    /// The listing page carried nothing. Not a fault.
    EmptyPageOrCatalogEnd,
    /// The network failed under us.
    NetworkError,
    /// The filesystem failed under us.
    IoError,
    /// The cache could not be read or written.
    CacheError,
    /// JSON we produced or consumed would not round-trip.
    JsonError,
    /// **The page loaded and we could not read it.** The scraper is broken and
    /// a human should look.
    ///
    /// The one code in this table worth paging on, which is exactly why
    /// [`ErrorKind::Internal`] exists beside it: the fork this taxonomy is
    /// ported from classified every unrecognised error as `parse_failed`, and
    /// an alarm that fires on everything is not an alarm.
    ParseFailed,
    /// Nothing above recognised this error. A bug in this tool.
    ///
    /// Deliberately outside the ranges above, and deliberately **not**
    /// `parse_failed`. `70` is `EX_SOFTWARE` in `sysexits(3)`: an internal
    /// software error, which is precisely what an error this tool cannot name
    /// about itself is.
    Internal,
}

impl ErrorKind {
    /// The stable string a caller branches on, as it appears in `--json`.
    pub fn error_type(self) -> &'static str {
        match self {
            ErrorKind::InvalidInput => "invalid_input",
            ErrorKind::BrowserLaunchFailed => "browser_launch_failed",
            ErrorKind::ChromeDownloadFailed => "chrome_download_failed",
            ErrorKind::NavigationTimeout => "navigation_timeout",
            ErrorKind::NavigationFailed => "navigation_failed",
            ErrorKind::CloudflareBlocked => "cloudflare_blocked",
            ErrorKind::ProductNotFound => "product_not_found",
            // `catalog`, not the fork's `catelog` (#9).
            ErrorKind::EmptyPageOrCatalogEnd => "empty_page_or_catalog_end",
            ErrorKind::NetworkError => "network_error",
            ErrorKind::IoError => "io_error",
            ErrorKind::CacheError => "cache_error",
            ErrorKind::JsonError => "json_error",
            ErrorKind::ParseFailed => "parse_failed",
            ErrorKind::Internal => "internal_error",
        }
    }

    /// The process exit code. Stable: callers branch on these numbers.
    pub fn exit_code(self) -> u8 {
        match self {
            ErrorKind::InvalidInput => 2,
            ErrorKind::BrowserLaunchFailed => 10,
            ErrorKind::ChromeDownloadFailed => 11,
            ErrorKind::NavigationTimeout => 20,
            ErrorKind::NavigationFailed => 21,
            ErrorKind::CloudflareBlocked => 22,
            ErrorKind::ProductNotFound => 23,
            ErrorKind::EmptyPageOrCatalogEnd => 24,
            ErrorKind::NetworkError => 30,
            ErrorKind::IoError => 31,
            ErrorKind::CacheError => 32,
            ErrorKind::JsonError => 40,
            ErrorKind::ParseFailed => 41,
            ErrorKind::Internal => 70,
        }
    }

    /// Every variant, so a sweep over the taxonomy cannot fall behind it.
    pub const ALL: &'static [ErrorKind] = &[
        ErrorKind::InvalidInput,
        ErrorKind::BrowserLaunchFailed,
        ErrorKind::ChromeDownloadFailed,
        ErrorKind::NavigationTimeout,
        ErrorKind::NavigationFailed,
        ErrorKind::CloudflareBlocked,
        ErrorKind::ProductNotFound,
        ErrorKind::EmptyPageOrCatalogEnd,
        ErrorKind::NetworkError,
        ErrorKind::IoError,
        ErrorKind::CacheError,
        ErrorKind::JsonError,
        ErrorKind::ParseFailed,
        ErrorKind::Internal,
    ];
}

impl IherbError {
    /// This error's place in the taxonomy.
    ///
    /// Exhaustive on purpose: a new [`IherbError`] variant stops the build here
    /// until someone decides what a caller should do about it. An error added
    /// without a decision would silently be [`ErrorKind::Internal`], which is
    /// the classification that means "this tool is broken".
    pub fn kind(&self) -> ErrorKind {
        match self {
            IherbError::InvalidInput(_) => ErrorKind::InvalidInput,
            IherbError::BrowserLaunch(_) => ErrorKind::BrowserLaunchFailed,
            IherbError::ChromeDownload(_) => ErrorKind::ChromeDownloadFailed,
            IherbError::Navigation(msg) => {
                if looks_like_a_timeout(msg) {
                    ErrorKind::NavigationTimeout
                } else {
                    ErrorKind::NavigationFailed
                }
            }
            IherbError::CloudflareBlocked(_) => ErrorKind::CloudflareBlocked,
            IherbError::ProductNotFound(_) => ErrorKind::ProductNotFound,
            IherbError::EmptyPageOrCatalogEnd(_) => ErrorKind::EmptyPageOrCatalogEnd,
            IherbError::ParseFailed(_) => ErrorKind::ParseFailed,
            // Not a code of its own, and not `parse_failed` either. `--currency`
            // named a currency this storefront does not price in, and the only
            // thing that changes the answer is a different flag — which is what
            // `invalid_input` tells a caller to do. The `message` names both
            // currencies, so nothing is lost by not spending a code on it.
            IherbError::CurrencyMismatch { .. } => ErrorKind::InvalidInput,
            IherbError::Cache(_) => ErrorKind::CacheError,
            IherbError::Network(_) => ErrorKind::NetworkError,
            IherbError::Io(_) => ErrorKind::IoError,
            IherbError::Json(_) => ErrorKind::JsonError,
        }
    }
}

/// Whether a navigation failure was the clock rather than the address.
///
/// A heuristic over the driver's own message, and the only signal available:
/// `IherbError::Navigation` wraps whatever `chromiumoxide` reported, which
/// carries the distinction in prose and not in a type. It is worth making
/// anyway — "the page was slow" and "the page will never load" call for
/// opposite responses, and collapsing them into one code hands a caller a
/// retry decision it cannot make.
fn looks_like_a_timeout(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("timeout") || m.contains("timed out") || m.contains("deadline")
}

/// Classify an error the way the process exit code and `--json` report it.
///
/// Walks the whole `anyhow` chain rather than looking only at the outermost
/// error, because every layer of the fetch pipeline adds a `.context(..)`: the
/// `IherbError` a caller needs is almost never on top. An error with no
/// `IherbError` anywhere in it is [`ErrorKind::Internal`] — **not**
/// [`ErrorKind::ParseFailed`], which is the one signal in the table that means
/// a human should look at the scraper, and which is worthless the moment every
/// unrecognised error is filed under it.
pub fn classify_error(error: &anyhow::Error) -> ErrorKind {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<IherbError>())
        .map(IherbError::kind)
        .unwrap_or(ErrorKind::Internal)
}
