//! #11: nothing is slept through before the page is looked at, and `--timing`
//! says where the time went.
//!
//! Every navigation used to open with `sleep(--delay)` — 2000 ms by default,
//! charged per navigation — before anything checked the page at all. The claim
//! this file has to decide is a claim about *elapsed time*, so every assertion
//! here is a measurement rather than a reading of the code.
//!
//! Two levels, deliberately:
//!
//!  - `wait_for_selectors` is driven by a probe the test supplies, so the
//!    "ready at once means returns at once" claim is decided offline, with no
//!    browser and no network;
//!  - one end-to-end test drives the real `Navigator` against a page served
//!    over loopback, because a sleep re-added to `navigate` rather than to the
//!    wait would slip past the offline test. It skips loudly without system
//!    Chrome rather than downloading one.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use iherb_cli::browser::session::BrowserSession;
use iherb_cli::config::{AppConfig, CacheMode, ProfileChoice, DEFAULT_CACHE_TTL, DEFAULT_DELAY_MS};
use iherb_cli::scraper::navigation::{
    wait_for_selectors, NavigationTiming, Navigator, Readiness, ReadinessTarget, READINESS_BUDGET,
    READINESS_POLL,
};

static ONE_AT_A_TIME: Mutex<()> = Mutex::const_new(());

/// The `--delay` the end-to-end test runs with.
///
/// Deliberately far above every default, so that "the delay is slept before
/// every navigation" and "it is not" are separated by seconds rather than by
/// milliseconds. See `a_real_navigation_no_longer_pays_the_delay_per_page`.
const SENTINEL_DELAY_MS: u64 = 3000;

/// **A page that is already there is read at once.**
///
/// The probe answers `true` on its first call, so the only thing between the
/// call and the return is the code under test. The old behaviour slept
/// `--delay` — 2000 ms — before looking; the bound here is a quarter of one
/// poll interval, which no sleep worth removing fits inside.
#[tokio::test]
async fn a_ready_page_is_not_waited_on() {
    let started = Instant::now();
    let outcome =
        wait_for_selectors(ReadinessTarget::Product.selectors(), |_| async { true }).await;
    let elapsed = started.elapsed();

    assert_eq!(
        outcome,
        Readiness::Ready("script[type=\"application/ld+json\"]"),
        "the first selector in the set is the one that should have answered"
    );
    assert!(
        elapsed < READINESS_POLL,
        "a page that is already ready must be read immediately, and this took \
         {:?}. The pre-#11 behaviour slept 2000 ms here before looking at the \
         page at all.",
        elapsed
    );
}

/// **A selector later in the set still answers on the first pass.**
///
/// The set is "any of these", so a page carrying only the fourth selector is as
/// ready as one carrying the first, and must not pay a poll interval per
/// selector to prove it.
#[tokio::test]
async fn any_selector_in_the_set_ends_the_wait_on_the_first_pass() {
    let target = "#product-overview";
    let started = Instant::now();
    let outcome = wait_for_selectors(ReadinessTarget::Product.selectors(), |s| async move {
        s == target
    })
    .await;
    let elapsed = started.elapsed();

    assert_eq!(outcome, Readiness::Ready(target));
    assert!(
        elapsed < READINESS_POLL,
        "every selector in a set is checked before any sleep, and this took {:?}",
        elapsed
    );
}

/// **An empty result set returns at once rather than waiting out the budget.**
///
/// `.no-results` is in the search set for exactly this: a search that genuinely
/// matches nothing renders a page that will never carry a product card, and
/// without a selector for the empty case it would burn the full eight seconds
/// to learn what the page said immediately.
#[tokio::test]
async fn an_empty_result_set_does_not_wait_out_the_budget() {
    let started = Instant::now();
    let outcome = wait_for_selectors(ReadinessTarget::Search.selectors(), |s| async move {
        s == ".no-results"
    })
    .await;
    let elapsed = started.elapsed();

    assert_eq!(outcome, Readiness::Ready(".no-results"));
    assert!(
        elapsed < READINESS_POLL,
        "an empty search must return as soon as the page says so, and this took {:?}",
        elapsed
    );
}

/// **A page that never becomes ready is read anyway, and is bounded.**
///
/// The budget is a bound on waiting, not a verdict: a selector set is a claim
/// about iHerb's markup, and iHerb changes it. Failing the run here would turn
/// every such change into an outage instead of a slow read, and the scrapers
/// below already have layered fallbacks and a `parse_failed` for a page that
/// truly cannot be read.
#[tokio::test]
async fn a_page_that_never_arrives_times_out_rather_than_failing() {
    let started = Instant::now();
    let outcome =
        wait_for_selectors(ReadinessTarget::Product.selectors(), |_| async { false }).await;
    let elapsed = started.elapsed();

    assert_eq!(
        outcome,
        Readiness::TimedOut,
        "a page that never shows its data is not an error here"
    );
    assert!(
        elapsed >= READINESS_BUDGET,
        "the budget must actually be spent before giving up, and this gave up \
         after {:?}",
        elapsed
    );
    assert!(
        elapsed < READINESS_BUDGET + Duration::from_secs(2),
        "the wait must be bounded by the budget, and this took {:?}",
        elapsed
    );
}

/// **A target with no selectors is told so**, rather than being waited on for
/// nothing.
#[tokio::test]
async fn a_target_with_no_selectors_says_so() {
    let outcome = wait_for_selectors(ReadinessTarget::DocumentComplete.selectors(), |_| async {
        true
    })
    .await;
    assert_eq!(outcome, Readiness::ReadyState);
}

/// **`--timing` reports the four phases the issue names, and the total.**
///
/// Asserted on the rendered line rather than on the struct, because the line is
/// what a caller reads: a phase that stopped being printed would still be
/// timed, and nobody would know.
#[test]
fn the_timing_line_carries_every_phase() {
    let timing = NavigationTiming {
        goto: Duration::from_millis(412),
        cloudflare_check: Duration::from_millis(7),
        wait_selector: Duration::from_millis(263),
        html_extract: Duration::from_millis(31),
    };

    let line = timing.render(
        "https://no.iherb.com/pr/x/12949",
        Readiness::Ready("h1#name"),
    );

    for expected in [
        "goto_ms=412",
        "cloudflare_check_ms=7",
        "wait_selector_ms=263",
        "html_extract_ms=31",
        "total_ms=713",
        "ready=h1#name",
        "url=https://no.iherb.com/pr/x/12949",
    ] {
        assert!(
            line.contains(expected),
            "the timing line is missing {}: {}",
            expected,
            line
        );
    }
}

/// **The delay default is the politeness figure, not the old page-load guess.**
///
/// 2000 ms was never a politeness number: it was slept before every navigation
/// as a guess at how long a page takes to render, and on a 25-product
/// comparison that was roughly a third of the wall clock. Readiness selectors
/// answer that question now, so what is left is the gap between requests.
#[test]
fn the_delay_default_dropped_with_the_sleep() {
    assert_eq!(DEFAULT_DELAY_MS, 500);

    // An empty config file of the test's own, named explicitly, so the answer
    // is the tool's default and not whatever `~/.config/iherb-cli/config.toml`
    // happens to say on the machine running the suite.
    let scratch = Scratch::new("delay-default");
    let empty = scratch.path().join("config.toml");
    std::fs::write(&empty, "[defaults]\n").expect("write an empty config");

    let config = AppConfig::load(&iherb_cli::cli::GlobalArgs {
        config: Some(empty.clone()),
        ..iherb_cli::cli::GlobalArgs::none()
    })
    .expect("defaults");
    assert_eq!(
        config.delay_ms, DEFAULT_DELAY_MS,
        "a run that passes no --delay gets the politeness default"
    );

    let stated = AppConfig::load(&iherb_cli::cli::GlobalArgs {
        delay: Some(1234),
        config: Some(empty),
        ..iherb_cli::cli::GlobalArgs::none()
    })
    .expect("an explicit delay");
    assert_eq!(
        stated.delay_ms, 1234,
        "--delay still says what the gap between requests is"
    );
}

// ---------------------------------------------------------------------------
// End to end: a real browser, a real page, and a stopwatch.
// ---------------------------------------------------------------------------

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
    candidates.iter().map(PathBuf::from).find(|p| p.exists())
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "iherb-cli-test-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A loopback origin serving one page that answers the product readiness set.
struct Origin {
    url: String,
    shutdown: Option<std::sync::mpsc::Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Origin {
    fn start(body: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("non-blocking listener");
            loop {
                if rx.try_recv().is_ok() {
                    return;
                }
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, body),
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            }
        });
        Origin {
            url,
            shutdown: Some(tx),
            handle: Some(handle),
        }
    }
}

impl Drop for Origin {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve(mut stream: TcpStream, body: &str) {
    let mut buf = [0u8; 1024];
    let _ = stream.read(&mut buf);
    let _ = stream.write_all(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .as_bytes(),
    );
    let _ = stream.flush();
}

/// **A real navigation of a ready page costs far less than the old floor.**
///
/// The page is served locally and carries `h1#name`, so it is ready the moment
/// it loads. `--delay` is set to **three seconds** — far above any default —
/// because the claim under test is that the delay is not charged per
/// navigation. A test that passed `delay_ms: 0` would pass with the
/// unconditional sleep still in place, and so would one that used the 500 ms
/// default, since 500 ms fits inside any bound loose enough not to be flaky.
/// The sentinel is what makes the two behaviours separable: with the old sleep
/// this navigation cannot come in under three seconds, and with the new one it
/// comes in under one.
#[tokio::test]
async fn a_real_navigation_no_longer_pays_the_delay_per_page() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let Some(chrome) = system_chrome() else {
        eprintln!("SKIPPED: no system Chrome; this test needs a real browser");
        return;
    };

    let origin = Origin::start(
        "<html><head><title>ready</title></head><body><h1 id=\"name\">Ready</h1></body></html>",
    );
    let scratch = Scratch::new("timing");
    let config = AppConfig {
        country: "us".to_string(),
        currency: None,
        cache_mode: CacheMode::Off,
        cache_ttl: DEFAULT_CACHE_TTL,
        // A sentinel, not a default. See the doc comment: this is the number
        // the old code slept before looking at the page, and the assertion
        // below is that it is not slept.
        delay_ms: SENTINEL_DELAY_MS,
        debug: false,
        headful: false,
        timing: false,
        browser_path: None,
        profile: ProfileChoice::Throwaway,
        cache_dir: scratch.path().join("cache"),
        data_dir: scratch.path().join("data"),
    };

    let session = BrowserSession::launch(chrome, &config)
        .await
        .expect("failed to launch the browser");
    let page = session.new_page().await.expect("failed to open a page");
    let navigator = Navigator::new(config.delay_ms, None, false);

    // Once to warm the connection, then measured: the first navigation of a
    // process pays Chrome's own start-up costs, which are not what is under
    // test here.
    let _ = navigator
        .navigate(&page, &origin.url, ReadinessTarget::Product)
        .await
        .expect("warm-up navigation failed");

    let started = Instant::now();
    let html = navigator
        .navigate(&page, &origin.url, ReadinessTarget::Product)
        .await
        .expect("navigation failed");
    let elapsed = started.elapsed();

    let _ = session.close().await;

    assert!(
        html.contains("id=\"name\""),
        "the navigation read something other than the page served: {}",
        html
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "a ready page took {:?} to navigate with --delay at {} ms. Before #11 \
         every navigation slept the delay before looking at the page, so this \
         could not have come in under {} ms.",
        elapsed,
        SENTINEL_DELAY_MS,
        SENTINEL_DELAY_MS
    );
}
