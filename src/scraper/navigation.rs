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
}

impl Navigator {
    pub fn new(delay_ms: u64, storefront: Option<Storefront>) -> Self {
        Self {
            delay_ms,
            storefront,
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
            // Written from scratch rather than merged into whatever is there,
            // which is right for the profile this runs in: every session gets a
            // fresh, empty user-data directory, so there is nothing to preserve
            // and a read-modify-write would only add a way to get it wrong.
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

    pub async fn navigate(&self, page: &Page, url: &str) -> Result<String, IherbError> {
        tracing::info!("Navigating to: {}", url);

        // Before the navigation, not after: the cookies are read by the server
        // that renders the page, so setting them on a page already fetched
        // would change nothing about the HTML we are about to read.
        self.request_storefront(page).await;

        page.goto(url)
            .await
            .map_err(|e| navigation_failure(format_args!("Failed to navigate to {}", url), e))?;

        // Wait for initial page load
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;

        // Wait for document.readyState === 'complete' (up to 10s)
        for _ in 0..20 {
            let ready = page
                .evaluate("document.readyState")
                .await
                .ok()
                .and_then(|v| v.into_value::<String>().ok())
                .unwrap_or_default();
            if ready == "complete" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // Check for and handle Cloudflare challenge
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

        let html = page
            .content()
            .await
            .map_err(|e| navigation_failure("Failed to get page content", e))?;

        Ok(html)
    }

    pub async fn navigate_with_retry(
        &self,
        page: &Page,
        url: &str,
        max_retries: u32,
    ) -> Result<String, IherbError> {
        let mut last_err = None;

        for attempt in 1..=max_retries + 1 {
            match self.navigate(page, url).await {
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
