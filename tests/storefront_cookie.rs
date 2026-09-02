//! What `--currency` puts on the wire (#5).
//!
//! `--currency` used to be a label the formatter applied to numbers iHerb had
//! already priced. It is now a *request*: iHerb carries the storefront
//! preference in the cookies its own header picker writes, so asking for a
//! currency means setting those cookies before the page is fetched. Nothing
//! about that is visible in the HTML that comes back — the page simply *is* the
//! storefront it is — so the only place to assert it is the cookie jar.
//!
//! The file has two halves and needs both. The construction tests pin the
//! cookie *values*, which were measured against the live site and are not
//! guessable; the browser test pins that [`Navigator::navigate`] actually
//! *sets* them, which no amount of asserting on a constructor can show.
//!
//! ## What was measured, and against what
//!
//! On 2026-08-31, from a Norwegian IP, against product 12949:
//!
//! | request | `COUNTRY_CODE` | `CURRENCY_CODE` | price |
//! |---|---|---|---|
//! | no `--currency` at all | `NO` | `NOK` | NOK 880.63 |
//! | `--country us --currency USD` | `US` | `USD` | $64.56 |
//! | `--country de --currency EUR` | `DE` | `EUR` | €76.57 |
//!
//! The first row is the control and it is why the country is in the cookie.
//! **iHerb geolocates by IP**: with no preference expressed, `www.iherb.com`
//! serves the Norwegian storefront in NOK, which is what `--country`'s own help
//! has always warned about. And a preference naming only a currency does not
//! survive — sending `scurcode` without `sccode` gets the whole cookie
//! discarded and rebuilt from geoip, with the site recording the disagreement
//! in a `geoip-ccl-mismatch` cookie. The pair is what works.

use std::path::PathBuf;

use tokio::sync::Mutex;

use iherb_cli::config::{AppConfig, ProfileChoice};
use iherb_cli::scraper::navigation::{Navigator, ReadinessTarget, Storefront};

fn config(country: &str, currency: Option<&str>) -> AppConfig {
    AppConfig {
        country: country.to_string(),
        currency: currency.map(str::to_string),
        cache_mode: iherb_cli::config::CacheMode::Off,
        cache_ttl: iherb_cli::config::DEFAULT_CACHE_TTL,
        delay_ms: 0,
        debug: false,
        headful: false,
        timing: false,
        browser_path: None,
        profile: ProfileChoice::Throwaway,
        cache_dir: std::env::temp_dir().join("iherb-cli-storefront-cookie-cache"),
        data_dir: std::env::temp_dir().join("iherb-cli-storefront-cookie-data"),
    }
}

/// The cookie values, exactly as measured. Changing any of them is a change to
/// what iHerb is asked for, and the only way to find out whether the new one
/// works is to ask the live site again.
#[test]
fn the_preference_cookies_carry_the_country_and_the_currency_together() {
    let cookies = Storefront {
        country: "no".to_string(),
        currency: "NOK".to_string(),
    }
    .cookies();

    assert_eq!(
        cookies,
        vec![
            (
                "iher-pref1",
                "lan=en-US&sccode=NO&scurcode=NOK&storeid=0".to_string()
            ),
            (
                "ih-preference",
                "country=NO&currency=NOK&language=en-US&store=0".to_string()
            ),
        ]
    );
}

/// The expensive half of what was measured: **a currency without a country is
/// discarded**.
///
/// Asserted as its own test because it is the one thing here that cost a live
/// experiment to learn and that a later simplification would most plausibly
/// undo. "The cookie only needs the currency; we already pick the country with
/// the subdomain" is an entirely reasonable thing to think, it is wrong, and
/// the way it is wrong is silent — iHerb rebuilds the preference from the
/// caller's IP and serves a storefront nobody asked for.
#[test]
fn a_currency_without_a_country_is_not_what_gets_sent() {
    let storefront = Storefront {
        country: "de".to_string(),
        currency: "EUR".to_string(),
    };

    for (name, value) in storefront.cookies() {
        assert!(
            value.contains("sccode=DE") || value.contains("country=DE"),
            "{} must name the country or iHerb discards the whole preference: {:?}",
            name,
            value
        );
        assert!(
            value.contains("EUR"),
            "{} must name the currency: {:?}",
            name,
            value
        );
    }
}

/// The country is sent the way the site sends it, upper-cased, while
/// `--country` and the subdomain are lower-case.
#[test]
fn the_country_is_upper_cased_for_the_cookie() {
    let cookies = Storefront {
        country: "no".to_string(),
        currency: "NOK".to_string(),
    }
    .cookies();
    assert!(cookies[0].1.contains("sccode=NO"));
    assert!(!cookies[0].1.contains("sccode=no"));
}

/// No `--currency`, no preference. A run that asks for nothing must not pin its
/// country to a cookie either: expressing a preference is what `--currency`
/// means, and doing it unasked would change what every other run fetches.
#[test]
fn a_run_that_asks_for_no_currency_expresses_no_preference() {
    assert_eq!(Storefront::requested(&config("no", None)), None);
    assert_eq!(
        Storefront::requested(&config("no", Some("NOK"))),
        Some(Storefront {
            country: "no".to_string(),
            currency: "NOK".to_string(),
        })
    );
}

// ---------------------------------------------------------------------------
// The half a constructor cannot show
// ---------------------------------------------------------------------------

/// One browser at a time. `BrowserSession::launch` names its profile directory
/// after the process id and the millisecond, so two launches in the same test
/// binary can pick the same name and Chrome refuses the second with a
/// `SingletonLock` error. `browser_lifecycle.rs` serialises for the same reason.
static ONE_AT_A_TIME: Mutex<()> = Mutex::const_new(());

fn system_chrome() -> Option<PathBuf> {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
    } else if cfg!(target_os = "linux") {
        &[
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
        ]
    } else {
        &[]
    };
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
}

/// Navigating with a storefront requested really does write the cookies, read
/// back out of the browser rather than out of the code that was supposed to
/// write them.
///
/// This is the test that fails if the `request_storefront` call is taken out of
/// `navigate`. Every other assertion in this file is about a value; this one is
/// about whether anything sends it.
///
/// Hermetic: the page never leaves a `data:` URL. Setting a cookie is a local
/// operation on Chrome's own jar, and the cookies are written against
/// `https://www.iherb.com` rather than against whatever page is open —
/// deliberately, because a freshly opened tab is `about:blank` and CDP refuses
/// to set a cookie against that. `cargo test` must not reach the network, and
/// this does not.
#[tokio::test]
async fn navigating_with_a_requested_storefront_sets_the_cookies() {
    use chromiumoxide::cdp::browser_protocol::network::GetCookiesParams;
    use iherb_cli::browser::session::BrowserSession;

    let _serial = ONE_AT_A_TIME.lock().await;
    let Some(chrome) = system_chrome() else {
        eprintln!("SKIPPED: no system Chrome; this test needs a real browser");
        return;
    };

    let cfg = config("no", Some("NOK"));
    let session = BrowserSession::launch(chrome, &cfg)
        .await
        .expect("failed to launch the browser");
    let page = session.new_page().await.expect("failed to open a page");

    let navigator = Navigator::new(0, Storefront::requested(&cfg), false);
    navigator
        .navigate(
            &page,
            "data:text/html,<title>storefront</title>",
            ReadinessTarget::DocumentComplete,
        )
        .await
        .expect("navigation failed");

    let jar = page
        .execute(GetCookiesParams {
            urls: Some(vec!["https://no.iherb.com/".to_string()]),
        })
        .await
        .expect("could not read the cookie jar")
        .result
        .cookies;

    let named = |name: &str| {
        jar.iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| {
                panic!(
                    "{} was never set; navigate() did not ask for the storefront. Jar: {:?}",
                    name,
                    jar.iter().map(|c| &c.name).collect::<Vec<_>>()
                )
            })
            .clone()
    };

    let pref = named("iher-pref1");
    assert_eq!(pref.value, "lan=en-US&sccode=NO&scurcode=NOK&storeid=0");
    assert_eq!(pref.domain, ".iherb.com");
    assert!(pref.secure, "the site's own picker sets this cookie secure");
    assert_eq!(pref.path, "/");

    let preference = named("ih-preference");
    assert_eq!(
        preference.value,
        "country=NO&currency=NOK&language=en-US&store=0"
    );
    assert_eq!(preference.domain, ".iherb.com");

    // Long-lived, like the site's own: a session cookie would be dropped
    // between the runs that share one browser profile.
    assert!(
        pref.expires > 0.0,
        "the preference must outlive the session: {:?}",
        pref.expires
    );

    session.close().await.expect("failed to close the browser");
}

/// The control for the test above: no `--currency`, no cookies. Without this,
/// that test is satisfied by code that sets the preference unconditionally,
/// which would silently pin every run's country.
#[tokio::test]
async fn navigating_without_a_requested_storefront_sets_nothing() {
    use chromiumoxide::cdp::browser_protocol::network::GetCookiesParams;
    use iherb_cli::browser::session::BrowserSession;

    let _serial = ONE_AT_A_TIME.lock().await;
    let Some(chrome) = system_chrome() else {
        eprintln!("SKIPPED: no system Chrome; this test needs a real browser");
        return;
    };

    let cfg = config("no", None);
    let session = BrowserSession::launch(chrome, &cfg)
        .await
        .expect("failed to launch the browser");
    let page = session.new_page().await.expect("failed to open a page");

    let navigator = Navigator::new(0, Storefront::requested(&cfg), false);
    navigator
        .navigate(
            &page,
            "data:text/html,<title>storefront</title>",
            ReadinessTarget::DocumentComplete,
        )
        .await
        .expect("navigation failed");

    let jar = page
        .execute(GetCookiesParams {
            urls: Some(vec!["https://no.iherb.com/".to_string()]),
        })
        .await
        .expect("could not read the cookie jar")
        .result
        .cookies;

    assert!(
        !jar.iter()
            .any(|c| c.name == "iher-pref1" || c.name == "ih-preference"),
        "a run that asked for no currency expressed a preference anyway: {:?}",
        jar.iter().map(|c| &c.name).collect::<Vec<_>>()
    );

    session.close().await.expect("failed to close the browser");
}
