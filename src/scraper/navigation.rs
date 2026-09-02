use crate::error::IherbError;
use chromiumoxide::cdp::browser_protocol::network::{CookieParam, TimeSinceEpoch};
use chromiumoxide::error::CdpError;
use chromiumoxide::Page;
use std::time::Duration;

const MAX_CLOUDFLARE_RETRIES: u32 = 3;
const CLOUDFLARE_WAIT_SECS: u64 = 12;
const CLOUDFLARE_TITLE_MARKERS: &[&str] = &["Just a moment", "Attention Required"];

/// The domain iHerb's own cookies are scoped to, so one preference covers every
/// storefront subdomain exactly as the site's own picker does.
const COOKIE_DOMAIN: &str = ".iherb.com";

/// The URL the preference cookies are written against.
///
/// A constant rather than the page we are about to visit. The cookie belongs to
/// [`COOKIE_DOMAIN`] whatever page is open, and CDP wants a URL only to hang
/// scheme and host defaults on — which the explicit domain then overrides. It
/// also has to be `http(s)`: CDP refuses to set a cookie against `about:blank`
/// or a `data:` URL, and `about:blank` is exactly what a freshly opened tab is.
const COOKIE_URL: &str = "https://www.iherb.com";

/// How long the site's own picker keeps the preference. Matched so that what we
/// write is indistinguishable from what a person clicking the picker writes.
const COOKIE_LIFETIME_DAYS: f64 = 365.0;

/// The language sub-key both preference cookies carry. We do not offer a
/// `--language` flag, and iHerb discards a preference that is missing pieces
/// (see [`Storefront`]), so this is the site's own default rather than a
/// choice of ours.
const COOKIE_LANGUAGE: &str = "en-US";

/// The storefront to ask iHerb for: a country and a currency, together (#5).
///
/// Together is not a convenience. **iHerb ignores a preference cookie that
/// names a currency without a country** — measured against the live site, not
/// assumed. Sending only `scurcode` from an IP iHerb geolocates elsewhere gets
/// the whole preference discarded and rebuilt from geoip; sending the complete
/// pair is honoured, and the site records the disagreement in a
/// `geoip-ccl-mismatch` cookie rather than acting on it. So the country travels
/// with the currency even though `--country` already picks the subdomain,
/// because a currency on its own does not survive the trip.
///
/// That geoip default is also why `--country`'s own help has always warned that
/// iHerb may override it: from a Norwegian IP, `https://www.iherb.com/pr/item/12949`
/// serves the Norwegian storefront in NOK. The pair is what stops it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Storefront {
    /// The `--country` code, e.g. `no`. Sent uppercased, as the site sends it.
    pub country: String,
    /// The `--currency` code, e.g. `NOK`.
    pub currency: String,
}

impl Storefront {
    /// The storefront a run is asking iHerb for, or `None` when `--currency`
    /// was not given.
    ///
    /// `None` rather than "the country on its own": with no currency to ask for
    /// there is no preference to express, and writing one anyway would pin
    /// every run's country to a cookie nobody asked for. That is a real
    /// difference, not a nicety — iHerb geolocates by IP, so a run that
    /// expresses no preference gets the storefront the IP suggests, which is
    /// what `--country`'s own help has always warned about.
    pub fn requested(config: &crate::config::AppConfig) -> Option<Self> {
        config.currency.clone().map(|currency| Self {
            country: config.country.clone(),
            currency,
        })
    }

    /// The cookies iHerb's own storefront picker writes, with the values it
    /// writes.
    ///
    /// `updateCCL()` — the handler behind the picker in the header — sets
    /// `iher-pref1` and `ih-preference` for 365 days and then reloads the page.
    /// The reload is the tell: the storefront is server-side state read off the
    /// request's cookies and rendered into `window.CURRENCY_CODE`, not
    /// something the page computes for itself. Nothing in the URL selects it —
    /// there is no query parameter — which is why this is done with cookies.
    ///
    /// Both cookies, because the site writes both. Each packs several
    /// preferences into one value as `k=v&k=v`, and the two spell the same two
    /// preferences differently: `iher-pref1` uses `sccode`/`scurcode`,
    /// `ih-preference` uses `country`/`currency`.
    pub fn cookies(&self) -> Vec<(&'static str, String)> {
        let country = self.country.to_uppercase();
        vec![
            (
                "iher-pref1",
                format!(
                    "lan={}&sccode={}&scurcode={}&storeid=0",
                    COOKIE_LANGUAGE, country, self.currency
                ),
            ),
            (
                "ih-preference",
                format!(
                    "country={}&currency={}&language={}&store=0",
                    country, self.currency, COOKIE_LANGUAGE
                ),
            ),
        ]
    }
}

/// How often a readiness probe is retried (#11).
///
/// 250 ms, from the fork. Short enough that a warm page — ready in about
/// 300 ms — is read almost as soon as it is ready, rather than after a fixed
/// two seconds.
pub const READINESS_POLL: Duration = Duration::from_millis(250);

/// The longest a readiness probe waits before giving up and reading the page
/// anyway.
///
/// Eight seconds. A *bound*, not a gate: see [`wait_for_selectors`].
pub const READINESS_BUDGET: Duration = Duration::from_secs(8);

/// How long `document.readyState` is polled for the targets that have no
/// selector to wait on. Unchanged from before #11.
const READY_STATE_BUDGET: Duration = Duration::from_secs(10);
const READY_STATE_POLL: Duration = Duration::from_millis(500);

/// What a page has to show before the pipeline reads it (#11).
///
/// Every navigation used to sleep for `--delay` — 2000 ms by default, charged
/// **per navigation** — and then poll `document.readyState` for up to another
/// ten seconds. `readyState == "complete"` fires when the document and its
/// subresources have loaded, which on a Next.js page says nothing about whether
/// the data being scraped is in the DOM, so the sleep was there to compensate
/// for a signal that answers the wrong question. Two seconds is simultaneously
/// too long for a warm page and too short for a cold one.
///
/// The replacement waits for a selector that proves *the data* has rendered.
/// Measured on a 25-product comparison against the Norwegian storefront, the
/// fixed delay was roughly a third of the wall clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessTarget {
    /// Nothing specific to wait for: poll `document.readyState` as before.
    ///
    /// Kept rather than removed because it is the honest answer for a page
    /// whose shape nothing here knows. A target that acquires a selector set
    /// moves off it.
    DocumentComplete,
    /// A product page.
    Product,
    /// A search or category results page.
    Search,
}

impl ReadinessTarget {
    /// The selectors any one of which proves this page's data has rendered.
    ///
    /// Any, not all: a product page carries JSON-LD *or* the DOM headings, and
    /// requiring both would wait out the budget on a page that is ready.
    ///
    /// Every selector here except `.no-results` is checked against a captured
    /// page in `tests/parsers/readiness.rs`, so a selector that stops matching
    /// the real site is a test failure rather than eight seconds of silence.
    /// **`.no-results` is the exception and is unverified**: this repository
    /// holds no capture of an empty result set, so it is carried from the fork
    /// on the fork's word. It costs nothing if it is wrong — see the bound
    /// below — and it is what makes an empty search return at once if it is
    /// right.
    pub fn selectors(self) -> &'static [&'static str] {
        match self {
            ReadinessTarget::DocumentComplete => &[],
            ReadinessTarget::Product => &[
                "script[type=\"application/ld+json\"]",
                "h1#name",
                "#product-specs-list",
                "#product-overview",
            ],
            ReadinessTarget::Search => &[
                "div.product-cell-container",
                "#product-count",
                // A genuinely empty result set. Without it an empty search
                // waits out the whole budget to learn what the page said
                // immediately.
                ".no-results",
            ],
        }
    }
}

/// How a readiness wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// This selector matched. Named so the log and `--timing` can say which.
    Ready(&'static str),
    /// Nothing matched inside [`READINESS_BUDGET`]. The page is read anyway.
    TimedOut,
    /// The target has no selectors; `document.readyState` was polled instead.
    ReadyState,
}

/// Wait until one of `selectors` is present, or the budget runs out.
///
/// # A bound, not a gate
///
/// A budget that expires is **not** an error and does not fail the run. A
/// selector set is a claim about the shape of iHerb's pages, and iHerb changes
/// them; a wait that failed the run would turn every such change into a hard
/// outage instead of a slow read, and the scrapers below already have layered
/// fallbacks and a `parse_failed` for the case where the page genuinely cannot
/// be read. The budget's job is to stop *waiting*, not to decide anything.
///
/// # Why the probe is a parameter
///
/// So the timing can be asserted without a browser. The claim #11 makes is that
/// nothing is slept through before the page is checked, and the only way to test
/// that is to hand this function a probe that answers immediately and measure
/// how long it takes to return. Driving it through `Page` would make that a
/// browser test, and a browser test that measures 300 ms against a 2000 ms
/// regression is one the suite cannot run offline.
pub async fn wait_for_selectors<F, Fut>(selectors: &[&'static str], mut probe: F) -> Readiness
where
    F: FnMut(&'static str) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    if selectors.is_empty() {
        return Readiness::ReadyState;
    }

    let deadline = std::time::Instant::now() + READINESS_BUDGET;
    loop {
        // Checked before any sleep, on every pass including the first. That is
        // the whole of "no unconditional sleep before content extraction": a
        // page that is already there costs one round trip per selector and
        // nothing else.
        for selector in selectors {
            if probe(selector).await {
                return Readiness::Ready(selector);
            }
        }
        if std::time::Instant::now() >= deadline {
            return Readiness::TimedOut;
        }
        tokio::time::sleep(READINESS_POLL).await;
    }
}

/// What each phase of one navigation cost, for `--timing` (#11).
///
/// Reported so the improvement is measurable rather than assumed, and because
/// an agent deciding whether a slow run is worth retrying wants to know which
/// phase was slow: a long `cloudflare_check_ms` and a long `wait_selector_ms`
/// call for opposite responses.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NavigationTiming {
    pub goto: Duration,
    pub cloudflare_check: Duration,
    pub wait_selector: Duration,
    pub html_extract: Duration,
}

impl NavigationTiming {
    /// Everything the four phases took together.
    pub fn total(&self) -> Duration {
        self.goto + self.cloudflare_check + self.wait_selector + self.html_extract
    }

    /// One line for stderr.
    ///
    /// `key=value` pairs rather than prose, because the reader is as likely to
    /// be an agent as a person, and milliseconds rather than a formatted
    /// duration for the same reason.
    pub fn render(&self, url: &str, readiness: Readiness) -> String {
        format!(
            "timing goto_ms={} cloudflare_check_ms={} wait_selector_ms={} \
             html_extract_ms={} total_ms={} ready={} url={}",
            self.goto.as_millis(),
            self.cloudflare_check.as_millis(),
            self.wait_selector.as_millis(),
            self.html_extract.as_millis(),
            self.total().as_millis(),
            match readiness {
                Readiness::Ready(selector) => selector,
                Readiness::TimedOut => "none-matched",
                Readiness::ReadyState => "document-complete",
            },
            url
        )
    }
}

pub struct Navigator {
    delay_ms: u64,
    /// The storefront to ask for, or `None` to take whatever iHerb serves.
    ///
    /// `Some` only when `--currency` was given: with no currency to ask for
    /// there is no preference to express, and writing one anyway would pin
    /// every run's country to a cookie nobody asked for.
    ///
    /// This is what makes `--currency` change the request rather than relabel
    /// the answer. It is a *request*, not a guarantee: iHerb decides whether to
    /// honour it, so nothing here assumes it worked. What the page came back
    /// saying is read off the page as it always was, and
    /// [`crate::targets::check_currency`] is what compares the two.
    storefront: Option<Storefront>,
    /// Whether `--timing` asked for the per-phase durations on stderr (#11).
    timing: bool,
}

impl Navigator {
    pub fn new(delay_ms: u64, storefront: Option<Storefront>, timing: bool) -> Self {
        Self {
            delay_ms,
            storefront,
            timing,
        }
    }

    /// Ask iHerb for [`Navigator::storefront`], the way its own header picker
    /// does: by setting the preference cookies before the request that reads
    /// them.
    ///
    /// Public so a test can watch it happen. What it does is invisible in the
    /// HTML that comes back — the page simply *is* the storefront it is — so
    /// the only way to assert it is to read the cookie jar it wrote into.
    ///
    /// Failing to set a cookie is logged and not fatal. It cannot be silent
    /// data corruption — a request that did not take comes back priced in the
    /// storefront's own currency, and that is exactly the disagreement
    /// `--currency` already errors on. Turning a cookie write into a hard
    /// failure would only replace a specific, accurate error with a vaguer one.
    pub async fn request_storefront(&self, page: &Page) {
        let Some(storefront) = self.storefront.as_ref() else {
            return;
        };

        // Chrome's clock, not ours, is what expires it.
        let expires = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() + COOKIE_LIFETIME_DAYS * 24.0 * 60.0 * 60.0)
            .unwrap_or(0.0);

        for (cookie, value) in storefront.cookies() {
            // Written from scratch rather than merged into whatever is there.
            // It was right when every session got a fresh, empty user-data
            // directory; since #12 a profile can be persistent, and it is still
            // right — CDP replaces a cookie of the same name, domain and path,
            // these two carry every sub-key the site's own picker writes, and a
            // preference the caller stated on this invocation is the one that
            // should win over whatever a previous run left. A read-modify-write
            // would only add a way to merge a stale storefront into a fresh
            // request.
            let mut param = CookieParam::new(cookie, value);
            param.url = Some(COOKIE_URL.to_string());
            param.domain = Some(COOKIE_DOMAIN.to_string());
            param.path = Some("/".to_string());
            param.secure = Some(true);
            param.expires = Some(TimeSinceEpoch::new(expires));

            if let Err(e) = page.set_cookie(param).await {
                tracing::warn!(
                    "Could not set {} to request {} prices: {}. The storefront will price in \
                     its own currency, and --currency will report the disagreement.",
                    cookie,
                    storefront.currency,
                    e
                );
            }
        }

        tracing::debug!(
            "Requested the {} storefront in {} via the preference cookies",
            storefront.country,
            storefront.currency
        );
    }

    /// Navigate, wait for the page to be worth reading, and return its HTML.
    ///
    /// # There is no sleep before the page is checked (#11)
    ///
    /// There used to be: `--delay`, 2000 ms by default, charged **per
    /// navigation**, before anything looked at the page at all. On a 25-product
    /// comparison against the Norwegian storefront that was roughly a third of
    /// the wall clock, against a warm page that is ready in about 300 ms — and
    /// it was still not enough for a cold one, which is why extraction
    /// sometimes fell through to the DOM scraper and returned partial data.
    ///
    /// `--delay` now does the job its name describes: politeness *between*
    /// requests, in [`Navigator::rate_limit_delay`]. It is not a guess at page
    /// load time any more, so its default drops to 500 ms.
    ///
    /// # The phase order is the order the answers arrive in
    ///
    /// Cloudflare is checked before the readiness selectors, not after. An
    /// interstitial is a complete, ready page that contains none of the
    /// selectors, so probing first would spend the whole readiness budget
    /// learning what the title says immediately.
    pub async fn navigate(
        &self,
        page: &Page,
        url: &str,
        readiness: ReadinessTarget,
    ) -> Result<String, IherbError> {
        tracing::info!("Navigating to: {}", url);

        // Before the navigation, not after: the cookies are read by the server
        // that renders the page, so setting them on a page already fetched
        // would change nothing about the HTML we are about to read.
        self.request_storefront(page).await;

        let mut timing = NavigationTiming::default();
        let started = std::time::Instant::now();

        page.goto(url)
            .await
            .map_err(|e| navigation_failure(format_args!("Failed to navigate to {}", url), e))?;
        timing.goto = started.elapsed();

        let cloudflare_started = std::time::Instant::now();
        let cloudflare = self.clear_cloudflare(page).await;
        timing.cloudflare_check = cloudflare_started.elapsed();
        cloudflare?;

        let wait_started = std::time::Instant::now();
        let outcome = self.wait_until_ready(page, readiness).await;
        timing.wait_selector = wait_started.elapsed();

        if outcome == Readiness::TimedOut {
            tracing::warn!(
                "None of the {:?} readiness selectors appeared within {:?}; reading the \
                 page as it stands. Extraction may fall through to its weaker strategies.",
                readiness,
                READINESS_BUDGET
            );
        }

        let extract_started = std::time::Instant::now();
        let html = page
            .content()
            .await
            .map_err(|e| navigation_failure("Failed to get page content", e))?;
        timing.html_extract = extract_started.elapsed();

        // stderr directly rather than through `tracing`, because `--timing` is
        // a request for these numbers and not a request to turn logging up:
        // routing it through the subscriber would make it arrive only under
        // `--debug`, or make `--timing` change the level of everything else.
        if self.timing {
            eprintln!("{}", timing.render(url, outcome));
        }
        tracing::debug!("{}", timing.render(url, outcome));

        Ok(html)
    }

    /// Wait for the page to show that the data being scraped has rendered.
    async fn wait_until_ready(&self, page: &Page, readiness: ReadinessTarget) -> Readiness {
        let selectors = readiness.selectors();
        if selectors.is_empty() {
            self.wait_for_ready_state(page).await;
            return Readiness::ReadyState;
        }
        wait_for_selectors(selectors, |selector| async move {
            page.find_element(selector).await.is_ok()
        })
        .await
    }

    /// The pre-#11 wait, kept for [`ReadinessTarget::DocumentComplete`].
    ///
    /// `readyState` is the wrong signal for a page whose data arrives after the
    /// document does, which is why the selectors exist — but for a target
    /// nothing here knows the shape of, it is the only signal there is, and it
    /// is better than reading the page mid-parse.
    async fn wait_for_ready_state(&self, page: &Page) {
        let deadline = std::time::Instant::now() + READY_STATE_BUDGET;
        loop {
            let ready = page
                .evaluate("document.readyState")
                .await
                .ok()
                .and_then(|v| v.into_value::<String>().ok())
                .unwrap_or_default();
            if ready == "complete" || std::time::Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(READY_STATE_POLL).await;
        }
    }

    /// Detect a Cloudflare interstitial and wait it out, up to
    /// [`MAX_CLOUDFLARE_RETRIES`] attempts.
    ///
    /// Lifted out of [`Navigator::navigate`] unchanged in behaviour, so that
    /// `--timing` can charge it its own phase rather than folding it into the
    /// readiness wait it precedes.
    async fn clear_cloudflare(&self, page: &Page) -> Result<(), IherbError> {
        for attempt in 1..=MAX_CLOUDFLARE_RETRIES {
            if !self.is_cloudflare_challenge(page).await {
                break;
            }

            if attempt == MAX_CLOUDFLARE_RETRIES {
                return Err(IherbError::CloudflareBlocked(MAX_CLOUDFLARE_RETRIES));
            }

            tracing::info!(
                "Cloudflare challenge detected (attempt {}/{}), waiting up to {}s...",
                attempt,
                MAX_CLOUDFLARE_RETRIES,
                CLOUDFLARE_WAIT_SECS
            );

            // Try clicking the Cloudflare Turnstile checkbox (may fail due to cross-origin, but worth trying)
            let _ = page
                .evaluate(
                    r#"
                    try {
                        const iframe = document.querySelector('iframe[src*="challenges"]');
                        if (iframe && iframe.contentDocument) {
                            const checkbox = iframe.contentDocument.querySelector('input[type="checkbox"]');
                            if (checkbox) checkbox.click();
                        }
                    } catch(e) {}
                    "#,
                )
                .await;

            // Wait for Cloudflare to resolve, but check periodically for early exit
            let check_interval_ms = 1000;
            let total_checks = (CLOUDFLARE_WAIT_SECS * 1000) / check_interval_ms;
            for _ in 0..total_checks {
                tokio::time::sleep(Duration::from_millis(check_interval_ms)).await;
                if !self.is_cloudflare_challenge(page).await {
                    tracing::info!("Cloudflare challenge resolved early");
                    break;
                }
            }
        }

        Ok(())
    }

    pub async fn navigate_with_retry(
        &self,
        page: &Page,
        url: &str,
        max_retries: u32,
        readiness: ReadinessTarget,
    ) -> Result<String, IherbError> {
        let mut last_err = None;

        for attempt in 1..=max_retries + 1 {
            match self.navigate(page, url, readiness).await {
                Ok(html) => return Ok(html),
                Err(e) => {
                    tracing::warn!(
                        "Navigation attempt {}/{} failed: {}",
                        attempt,
                        max_retries + 1,
                        e
                    );
                    last_err = Some(e);
                    if attempt <= max_retries {
                        let backoff = Duration::from_secs(2u64.pow(attempt - 1));
                        tracing::info!("Retrying in {:?}...", backoff);
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }

        Err(last_err.unwrap())
    }

    async fn is_cloudflare_challenge(&self, page: &Page) -> bool {
        match page.evaluate("document.title").await {
            Ok(val) => {
                let title = val.into_value::<String>().unwrap_or_default();
                CLOUDFLARE_TITLE_MARKERS
                    .iter()
                    .any(|marker| title.contains(marker))
            }
            Err(_) => false,
        }
    }

    pub async fn rate_limit_delay(&self) {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
    }
}

/// Name a navigation failure while `chromiumoxide`'s own error is still typed.
///
/// This is the only place the timeout distinction can honestly be made. The
/// classifier used to make it *afterwards*, by lower-casing the flattened
/// message and looking for "timeout", "timed out" or "deadline" — and that
/// message embeds the URL the run asked for. So `iherb-cli search timeout`
/// built a URL containing the word, and every failure of that navigation,
/// whatever its cause, reported `navigation_timeout` (20) and told the caller
/// to retry. A caller's retry decision must not be steerable by the caller's
/// own query text. The heuristic failed in the other direction too: one wording
/// change upstream and real timeouts would have started reporting as 21, with
/// nothing to notice it.
///
/// [`CdpError::Timeout`] is the driver's own answer to the same question —
/// `chromiumoxide` maps its internal `NavigationError::Timeout` onto it — and
/// [`CdpError::LaunchTimeout`] is the same fact about the launch handshake.
pub fn navigation_failure(context: impl std::fmt::Display, error: CdpError) -> IherbError {
    match error {
        CdpError::Timeout | CdpError::LaunchTimeout(_) => {
            IherbError::NavigationTimeout(format!("{}: {}", context, error))
        }
        _ => IherbError::Navigation(format!("{}: {}", context, error)),
    }
}
