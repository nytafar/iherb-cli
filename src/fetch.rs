//! The fetch pipeline.
//!
//! `search` and `product` used to be the same procedure written twice: validate
//! input, look in the cache, launch a browser, open a page, navigate with retry,
//! extract, validate, store. That block now lives here exactly once, and a
//! command is a [`FetchTarget`] descriptor rather than another copy of it.

use anyhow::{Context, Result};
use chromiumoxide::Page;
use serde::{de::DeserializeOwned, Serialize};
use std::future::Future;
use std::time::SystemTime;

use crate::browser::session::BrowserSession;
use crate::cache::{Cache, CacheKey};
use crate::config::AppConfig;
use crate::scraper::navigation::{Navigator, Storefront};

/// Navigation attempts after the first, per page.
const NAVIGATION_RETRIES: u32 = 2;

/// What the pipeline waits for before reading a page's HTML.
///
/// Every target waits the same way today: [`Navigator`] sleeps for the
/// configured delay, then polls `document.readyState`. #11 replaces that with a
/// per-target readiness probe, and this is the seam it plugs into. Until then
/// there is one variant and behaviour is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessTarget {
    /// Wait for `document.readyState === "complete"`.
    DocumentComplete,
}

/// Whether the pipeline should walk another page of a paginated target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paging {
    /// The last page said there is nothing after it.
    Done,
    /// There may be more; fetch the next page if the target has one.
    More,
}

/// Where the data in a document came from, and when (#44).
///
/// An enum rather than a `(SystemTime, bool)` pair, and that is what makes
/// #44's freshness criterion true instead of coincidental. A fresh record's
/// page was read *during this run*, so there is no second instant to record and
/// nothing to drift: `fetched_at` and `emitted_at` come out of one clock sample
/// because there is only one. The pair could not do that — the fresh fetch
/// stamped `SystemTime::now()` before the browser was shut down, `emitted_at`
/// sampled the clock again after it, and a run that crossed a second boundary
/// serialized two different timestamps for what the README calls the same
/// instant.
///
/// A cache hit is the case with a real second instant, and it keeps one: the
/// mtime of the file. That is the distinction the two fields exist for. A price
/// read three weeks ago is not stale, it is wrong (#25).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Read from iHerb during this run.
    Fresh,
    /// Read from a cache file written at this instant.
    Cached(SystemTime),
}

impl Provenance {
    /// When the page was read, for a run that reports itself at `emitted_at`.
    pub fn fetched_at(self, emitted_at: SystemTime) -> SystemTime {
        match self {
            Provenance::Fresh => emitted_at,
            Provenance::Cached(written_at) => written_at,
        }
    }

    /// Whether the page was read off the local cache rather than off iHerb.
    pub fn from_cache(self) -> bool {
        matches!(self, Provenance::Cached(_))
    }
}

/// A fetched artefact and where it came from. Commands print the latter as the
/// "Data from" line, and `--json` puts it in the envelope.
#[derive(Debug)]
pub struct Fetched<T> {
    pub data: T,
    pub provenance: Provenance,
}

impl<T> Fetched<T> {
    /// Read from iHerb, by this run. The only constructor the fetch pipeline
    /// uses for a fresh result, so a test that asserts on what this produces is
    /// asserting on the production path rather than on its own fixture.
    pub fn fresh(data: T) -> Self {
        Self {
            data,
            provenance: Provenance::Fresh,
        }
    }

    /// Read from a cache file written at `written_at`.
    pub fn cached(data: T, written_at: SystemTime) -> Self {
        Self {
            data,
            provenance: Provenance::Cached(written_at),
        }
    }
}

/// A failed run, and whatever the envelope can still say about where it got to.
///
/// The provenance travels with the error because #44 defines `fetched_at` as
/// when the page was read, and the same envelope wraps errors. A page that
/// loaded and then failed validation — `parse_failed`, `currency_mismatch`,
/// `product_not_found` — used to report `fetched_at: null` and
/// `from_cache: null`, which states, of a page that was read, that none was.
/// The `?` operator loses that fact, so it is carried rather than inferred.
#[derive(Debug)]
pub struct Failure {
    pub error: anyhow::Error,
    /// `Some` exactly when a page was read before the failure.
    pub provenance: Option<Provenance>,
}

impl Failure {
    /// A failure that happened *after* a page had been read: extraction,
    /// assembly or validation rejecting what came back.
    pub fn after_page_read(error: impl Into<anyhow::Error>) -> Self {
        Self {
            error: error.into(),
            provenance: Some(Provenance::Fresh),
        }
    }
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        Self {
            error,
            provenance: None,
        }
    }
}

impl From<crate::error::IherbError> for Failure {
    fn from(error: crate::error::IherbError) -> Self {
        Self {
            error: error.into(),
            provenance: None,
        }
    }
}

/// One thing the pipeline knows how to fetch.
///
/// A target says where its data lives in the cache, which URLs to visit, how to
/// read a visited page, and what counts as a usable result. It never launches a
/// browser, navigates, retries or touches the cache — [`fetch`] does all of that.
pub trait FetchTarget {
    /// The value that gets cached and returned.
    type Output: Serialize + DeserializeOwned;

    /// State gathered across pages before it becomes an [`Self::Output`].
    /// Single-page targets typically use `Option<T>`.
    type Accumulator: Default;

    /// Where this target's data lives in the cache.
    fn cache_key(&self) -> CacheKey;

    /// The URL for a given 1-based page number.
    fn url(&self, page_num: usize) -> String;

    /// Upper bound on pages walked. The target can stop earlier by returning
    /// [`Paging::Done`] from [`Self::extract`] or `true` from [`Self::has_enough`].
    fn page_count(&self) -> usize {
        1
    }

    /// What the pipeline waits for before reading each page. See #11.
    fn readiness(&self) -> ReadinessTarget {
        ReadinessTarget::DocumentComplete
    }

    /// Context added to a navigation failure, e.g. "Failed to navigate to the
    /// product page".
    fn navigation_context(&self) -> String;

    /// Whether enough has already been gathered to skip the next navigation.
    fn has_enough(&self, _acc: &Self::Accumulator) -> bool {
        false
    }

    /// Read one navigated page into `acc` and say whether to keep paging.
    ///
    /// Returned as an explicit `impl Future` rather than an `async fn` so the
    /// `Send` bound is visible — #10 needs to drive these concurrently.
    fn extract<'a>(
        &'a self,
        page: &'a Page,
        html: &'a str,
        acc: &'a mut Self::Accumulator,
    ) -> impl Future<Output = Result<Paging>> + Send + 'a;

    /// Assemble what was gathered into the value to cache and return.
    fn finish(&self, acc: Self::Accumulator) -> Result<Self::Output>;

    /// Reject an output that extraction produced but that is not real data.
    /// Runs before the cache store, so a rejected result is never cached.
    fn validate(&self, out: &Self::Output) -> Result<()>;

    /// Whether an entry found in the cache answers *this* request.
    ///
    /// The cache key says which requests share an entry; this says whether the
    /// entry that was found is enough for the one asking. They are different
    /// questions, and #6 is what happens when only the first is asked: two
    /// searches differing only in `--limit` share an entry on purpose, because
    /// the entry holds everything either run fetched — but a `--limit 200`
    /// request reading what a `--limit 10` run left behind was handed 48
    /// products and told nothing.
    ///
    /// Answering `false` makes the pipeline refetch, exactly as a miss would.
    /// A target whose entries always answer every request keeps the default.
    fn cache_is_sufficient(&self, _cached: &Self::Output) -> bool {
        true
    }
}

/// Fetch a target: cache lookup, lazy browser launch, navigation with retry,
/// extraction and cache store.
///
/// The browser is launched only if the cache misses, and `browser_session` is
/// reused across calls so a batch shares one browser.
pub async fn fetch<T: FetchTarget>(
    target: &T,
    config: &AppConfig,
    browser_session: &mut Option<BrowserSession>,
) -> Result<Fetched<T::Output>, Failure> {
    // The cache lookup comes first and nothing above the launch below touches
    // the browser: a cache hit must never start Chrome.
    if let Some(hit) = cached(target, config) {
        return Ok(hit);
    }

    let session = get_or_launch_browser(config, browser_session).await?;
    fetch_on(target, config, session).await
}

/// The cache half of [`fetch`]: the stored entry for this target, if there is
/// one and it answers the request.
///
/// Split out so the decision is reachable without a browser — the one in
/// [`FetchTarget::cache_is_sufficient`] is behaviour, and behaviour that only
/// runs inside a function that launches Chrome is behaviour nothing tests.
pub fn cached<T: FetchTarget>(target: &T, config: &AppConfig) -> Option<Fetched<T::Output>> {
    let cache = Cache::new(
        config.cache_dir.clone(),
        config.cache_mode,
        config.cache_ttl,
    );
    let key = target.cache_key();
    let hit = cache.get::<T::Output>(&key)?;

    if !target.cache_is_sufficient(&hit.data) {
        tracing::info!(
            "Cached {} does not answer this request; refetching",
            key.label()
        );
        return None;
    }

    Some(Fetched::cached(hit.data, hit.cached_at))
}

/// Fetch a target on a session that is already running, without consulting the
/// cache first.
///
/// This is the half of [`fetch`] that needs a browser. It takes `&BrowserSession`
/// rather than `&mut Option<BrowserSession>`, so several of these can run against
/// one session at once -- which is what #10's batch fetch with `--concurrency`
/// needs, and why [`FetchTarget::extract`] declares a `Send` future.
///
/// Callers that want the cache consulted want [`fetch`]. A result is still
/// *stored* here, so a batch populates the cache exactly as a single fetch does.
pub async fn fetch_on<T: FetchTarget>(
    target: &T,
    config: &AppConfig,
    session: &BrowserSession,
) -> Result<Fetched<T::Output>, Failure> {
    let page = session.new_page().await?;

    // The tab is closed on the way out of *both* arms, which is why the work is
    // a separate call rather than the body of this one: a `?` anywhere in there
    // used to return past the close, and a single fetch never noticed because
    // the process exited and Chrome died with it. #10 runs N targets over one
    // shared session, where every leaked tab is a live renderer process that
    // stays live (#45).
    let result = read_target(target, config, &page).await;

    if let Err(e) = page.close().await {
        // A tab that will not close is not a reason to throw away a good
        // result; it is a reason to say so.
        tracing::warn!("Failed to close page: {}", e);
    }

    result
}

/// Navigate, extract and cache one target on an already-open page.
///
/// Split out of [`fetch_on`] so that the page it was handed can be closed on
/// every path out, including the error ones. It owns nothing and closes
/// nothing.
async fn read_target<T: FetchTarget>(
    target: &T,
    config: &AppConfig,
    page: &Page,
) -> Result<Fetched<T::Output>, Failure> {
    let cache = Cache::new(
        config.cache_dir.clone(),
        config.cache_mode,
        config.cache_ttl,
    );
    let key = target.cache_key();
    // The storefront goes to the navigator because asking for one is part of
    // making the request, not part of reading the answer: iHerb carries the
    // preference in a cookie that has to be set before the page is fetched (#5).
    let navigator = Navigator::new(config.delay_ms, Storefront::requested(config));

    // Exhaustive so that #11 has to decide what a new variant means here.
    // `Navigator` already implements DocumentComplete.
    match target.readiness() {
        ReadinessTarget::DocumentComplete => {}
    }

    let page_count = target.page_count();
    let mut acc = T::Accumulator::default();

    for page_num in 1..=page_count {
        if target.has_enough(&acc) {
            break;
        }

        let url = target.url(page_num);
        let html = navigator
            .navigate_with_retry(page, &url, NAVIGATION_RETRIES)
            .await
            .context(target.navigation_context())?;

        // Everything from here on happens with a page already read, so a
        // failure carries that fact rather than reporting `fetched_at: null`
        // about a page that loaded (#44). `product_not_found` is raised in
        // `extract`, and `parse_failed` and `currency_mismatch` in `validate`.
        if target
            .extract(page, &html, &mut acc)
            .await
            .map_err(Failure::after_page_read)?
            == Paging::Done
        {
            break;
        }

        if page_num < page_count {
            navigator.rate_limit_delay().await;
        }
    }

    let out = target.finish(acc).map_err(Failure::after_page_read)?;
    target.validate(&out).map_err(Failure::after_page_read)?;

    // Logged, not propagated, and [`crate::cache::CacheWriteFailed`] is typed so
    // that it cannot be: a full disk is not a reason to throw away a page we
    // already fetched.
    if let Err(e) = cache.set(&key, &out) {
        tracing::debug!("Failed to cache {}: {}", key.label(), e);
    }

    // No clock call: a fresh record was read during this run, and the run's one
    // clock sample is taken when the document is rendered. That is what makes
    // `fetched_at == emitted_at` true for a fresh document rather than nearly
    // true (#44). See [`Provenance`].
    Ok(Fetched::fresh(out))
}

/// Launch the browser on first use and reuse it afterwards.
pub async fn get_or_launch_browser<'a>(
    config: &AppConfig,
    session: &'a mut Option<BrowserSession>,
) -> Result<&'a BrowserSession> {
    if session.is_none() {
        let chrome_path =
            crate::browser::resolve::resolve_chrome(config.browser_path.as_ref(), &config.data_dir)
                .await
                .context("Failed to resolve Chrome browser")?;

        let launched = BrowserSession::launch(chrome_path, config)
            .await
            .context("Failed to launch browser")?;

        *session = Some(launched);
    }
    Ok(session.as_ref().unwrap())
}
