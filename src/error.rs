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

    /// A navigation that ran out of time rather than out of road.
    ///
    /// Separate from [`IherbError::Navigation`] because the two call for
    /// opposite responses — retry the slow page, stop retrying the one that
    /// will never load — and because the distinction is *typed at the boundary*
    /// rather than read out of prose. It used to be neither: `classify_error`
    /// grepped the driver's message for "timeout", and that message embeds the
    /// URL we asked for, so searching iHerb for the literal query `timeout`
    /// made every navigation failure report itself as a timeout. User input
    /// must not be able to steer a caller's retry decision. See
    /// [`crate::scraper::navigation::navigation_failure`], which does the
    /// classification while `chromiumoxide`'s own typed error is still in hand.
    #[error("Browser navigation timed out: {0}")]
    NavigationTimeout(String),

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

    /// Chrome could not be obtained: the version index, the download, the
    /// archive, or writing any of it to disk.
    ///
    /// One variant over all of those on purpose. The taxonomy used to carry a
    /// `network_error` and an `io_error` beside this and construct neither, and
    /// the reason it constructed neither is that there is nothing for them to
    /// mean here: a socket that closed and a disk that filled are both, to a
    /// caller, "this machine could not get Chrome". See [`ErrorKind`].
    #[error("Chrome download failed: {0}")]
    ChromeDownload(String),
}

/// What went wrong, as a caller has to act on it (#9).
///
/// One variant per documented `error_type`, each with a stable exit code. The
/// point of the type is that the failures a caller must respond to differently
/// — skip this id, retry later, fix the environment, file a bug — are different
/// numbers rather than one `1`.
///
/// The codes are grouped by what a caller does about them: `2` is the caller's
/// input, `1x` the local environment, `2x` the page, `4x` the data,
/// [`ErrorKind::Internal`] is this tool's own bug, and
/// [`ErrorKind::Interrupted`] is the operator.
///
/// # Every code here has a producer
///
/// That is the invariant, and it is the whole of #9: a table that documents a
/// distinction the code cannot make is worse than no table, because a caller
/// branches on a code that never arrives and never notices. Four codes were
/// documented without one and have been removed rather than given a decorative
/// producer:
///
///  - **`network_error` (30)**. `reqwest` is used in exactly one place, the
///    Chrome download. A network failure there *is*
///    [`ErrorKind::ChromeDownloadFailed`]. Every other network failure this
///    tool has happens inside Chrome and already arrives as a navigation or
///    Cloudflare error.
///  - **`io_error` (31)**. Every filesystem failure sits inside a larger
///    operation that already has a code — the profile directory belongs to
///    [`ErrorKind::BrowserLaunchFailed`], the unpacked archive to
///    [`ErrorKind::ChromeDownloadFailed`] — or is not fatal at all.
///  - **`cache_error` (32)**. The cache is an optimization. A read that fails
///    is a miss and a write that fails is a log line; neither fails a run, and
///    neither should. See [`crate::cache::CacheWriteFailed`], which is
///    deliberately not an [`IherbError`] so that it cannot become one.
///  - **`json_error` (40)**. This tool consumes no JSON from a caller, and the
///    only JSON it produces is a record it already holds. A record that will
///    not serialize is a bug in this tool, which is what
///    [`ErrorKind::Internal`] already means.
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
    /// The storefront priced in a currency `--currency` did not allow.
    ///
    /// Its own code rather than [`ErrorKind::InvalidInput`], which is where it
    /// used to land. `--country us --currency CHF` is a syntactically perfect
    /// command line: it launches a browser, fetches a page, and can only fail
    /// once the storefront has answered. A caller that sees `invalid_input`
    /// re-reads its arguments; a caller that sees this one changes what it
    /// expects of the storefront. Sharing a code forced it to parse `message`
    /// to tell which, which is the taxonomy failing at its one job.
    CurrencyMismatch,
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
    /// Ctrl+C. Not a failure of anything, and the only code here the tool did
    /// not decide on its own.
    ///
    /// `130` is `128 + SIGINT`, which is what a shell reports for a process
    /// killed by an interrupt, and it was already this tool's interrupt exit —
    /// it simply had no `error_type` and, under `--json`, wrote no document at
    /// all. A caller that got 130 and zero bytes had nothing to parse in the
    /// one case where "always one document" mattered most.
    Interrupted,
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
            ErrorKind::CurrencyMismatch => "currency_mismatch",
            ErrorKind::ParseFailed => "parse_failed",
            ErrorKind::Internal => "internal_error",
            ErrorKind::Interrupted => "interrupted",
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
            ErrorKind::CurrencyMismatch => 25,
            ErrorKind::ParseFailed => 41,
            ErrorKind::Internal => 70,
            ErrorKind::Interrupted => 130,
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
        ErrorKind::CurrencyMismatch,
        ErrorKind::ParseFailed,
        ErrorKind::Internal,
        ErrorKind::Interrupted,
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
            IherbError::Navigation(_) => ErrorKind::NavigationFailed,
            IherbError::NavigationTimeout(_) => ErrorKind::NavigationTimeout,
            IherbError::CloudflareBlocked(_) => ErrorKind::CloudflareBlocked,
            IherbError::ProductNotFound(_) => ErrorKind::ProductNotFound,
            IherbError::EmptyPageOrCatalogEnd(_) => ErrorKind::EmptyPageOrCatalogEnd,
            IherbError::ParseFailed(_) => ErrorKind::ParseFailed,
            IherbError::CurrencyMismatch { .. } => ErrorKind::CurrencyMismatch,
        }
    }
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
