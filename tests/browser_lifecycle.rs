//! What the browser actually does, observed rather than read.
//!
//! The bugs this file guards (#45, #46, #47) were all invisible to code review:
//! the builder call *said* `("headless", "new")` and the fetch pipeline *said*
//! `session.new_page()`, and neither said what Chrome received, how many tabs
//! were left behind, or what stayed on disk. So every assertion here is made
//! against something the process did — an argv vector a real executable was
//! handed, a tab list read back over CDP, a directory that is or is not still
//! there — and never against the call that was supposed to produce it.
//!
//! #46's other half, the Ctrl+C path, is not here. Signal handling is
//! process-global and `ctrlc::set_handler` may be called once, so a test cannot
//! own it; it was verified by interrupting real runs and reading the process
//! list and the temp directory, before and after. The commit message records
//! both readings.
//!
//! `--headless` is asserted through a stub executable rather than through
//! Chrome itself. The stub is not a mock of the builder: it is a real program
//! that `Browser::launch` really execs, so what it writes down is the exact
//! argv vector Chrome would have received. That is the ground truth #47 needed
//! and the builder call could not give.
//!
//! The tab-count test needs a real browser and skips loudly when there is no
//! system Chrome, rather than downloading one: `cargo test` must not reach the
//! network.

use std::path::{Path, PathBuf};

use tokio::sync::Mutex;

use iherb_cli::browser::session::BrowserSession;
use iherb_cli::cache::CacheKey;
use iherb_cli::config::AppConfig;
use iherb_cli::fetch::{fetch_on, FetchTarget, Paging};

/// A config that touches nothing on disk that matters: caching off, no delay,
/// cache and data directories under a temp path of the test's own.
fn test_config(scratch: &Path, debug: bool) -> AppConfig {
    AppConfig {
        country: "us".to_string(),
        // #5 made this Option<String>; None is the new default and asserts nothing,
        // which is what a browser-lifecycle test wants.
        currency: None,
        no_cache: true,
        delay_ms: 0,
        debug,
        browser_path: None,
        cache_dir: scratch.join("cache"),
        data_dir: scratch.join("data"),
    }
}

/// A temp directory that removes itself, including when an assertion panics on
/// the way past it. A test that leaks a directory on failure is a test that
/// makes the next failure harder to read.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "iherb-cli-test-{}-{}-{}",
            name,
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

/// Every test here reads process-wide state — the temp directory, and a real
/// browser's tab list — so they run one at a time. Two of them concurrently
/// would each see the other's profile directory as its own leak.
static ONE_AT_A_TIME: Mutex<()> = Mutex::const_new(());

/// The `iherb-cli-<pid>-<millis>` profile directories that exist right now.
fn profile_dirs() -> Vec<PathBuf> {
    let temp = std::env::temp_dir();
    let Ok(entries) = std::fs::read_dir(&temp) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("iherb-cli-") && !n.starts_with("iherb-cli-test-"))
        })
        .collect()
}

/// Write an executable that records the argv it was handed and exits.
///
/// `Browser::launch` will fail against it — it never prints a DevTools URL —
/// but it fails *after* the exec, which is the only part this is asking about.
fn argv_recorder(dir: &Path) -> (PathBuf, PathBuf) {
    let exe = dir.join("fake-chrome.sh");
    let argv_file = dir.join("argv.txt");
    std::fs::write(
        &exe,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            argv_file.display()
        ),
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    (exe, argv_file)
}

/// Launch against the recorder and return the argv Chrome would have received.
async fn captured_argv(scratch: &Path, debug: bool) -> Vec<String> {
    let (exe, argv_file) = argv_recorder(scratch);
    let before = profile_dirs();

    // Expected to fail: the recorder is not a browser. The argv it wrote down
    // on its way out is the whole point.
    let _ = BrowserSession::launch(exe, &test_config(scratch, debug)).await;

    // #46: a launch that fails still created a profile directory, and nothing
    // else will ever clean it up — `close` and `Drop` both belong to a session
    // that in this case does not exist. Every call here fails, so every call
    // here is that path.
    let left_behind: Vec<PathBuf> = profile_dirs()
        .into_iter()
        .filter(|d| !before.contains(d))
        .collect();
    for dir in &left_behind {
        let _ = std::fs::remove_dir_all(dir);
    }
    assert!(
        left_behind.is_empty(),
        "a failed launch left its profile directory behind: {:?}",
        left_behind
    );

    let recorded = std::fs::read_to_string(&argv_file).unwrap_or_else(|e| {
        panic!(
            "the launch never reached the executable, so no argv was captured: {}",
            e
        )
    });
    recorded.lines().map(|l| l.to_string()).collect()
}

/// The launch really happened and the recording really is this launch's.
///
/// Without this, "no `--headless` in the argv" would pass just as happily on an
/// empty file, which is the shape of regression test this programme keeps
/// shipping by accident.
fn assert_argv_is_ours(argv: &[String]) {
    assert!(
        argv.iter().any(|a| a.starts_with("--user-data-dir=")
            && a.contains("iherb-cli-")
            && !a.contains("iherb-cli-test-")),
        "captured argv is not from a BrowserSession launch: {:?}",
        argv
    );
    assert!(
        argv.iter()
            .any(|a| a == "--disable-blink-features=AutomationControlled"),
        "captured argv is missing the switches BrowserSession sets: {:?}",
        argv
    );
}

fn headless_args(argv: &[String]) -> Vec<&String> {
    argv.iter()
        .filter(|a| *a == "--headless" || a.starts_with("--headless="))
        .collect()
}

/// #47. `--debug` is documented as "run browser in headed mode", and until #47
/// it did not: `HeadlessMode::True` is chromiumoxide's builder default, nothing
/// called `with_head()`, and real Chrome argv for a `--debug` run contained a
/// bare `--headless`. Observed on 2026-08-31 against system Chrome before the
/// fix; this is the same observation, made from a test.
#[tokio::test]
async fn debug_launches_a_headful_browser() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let scratch = Scratch::new("headful");
    let argv = captured_argv(scratch.path(), true).await;

    assert_argv_is_ours(&argv);
    assert!(
        headless_args(&argv).is_empty(),
        "--debug must not put Chrome in headless mode, but argv carried {:?}",
        headless_args(&argv)
    );
}

/// The other half of #47: fixing `--debug` must not quietly un-headless the
/// default path, which is every non-interactive run this tool exists for.
///
/// `--headless=new` exactly once, not a bare `--headless` and not the
/// `----headless=new` that #36 found 0.9 would have produced from a switch
/// passed as one string.
#[tokio::test]
async fn a_default_run_is_still_headless_new() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let scratch = Scratch::new("headless");
    let argv = captured_argv(scratch.path(), false).await;

    assert_argv_is_ours(&argv);
    assert_eq!(
        headless_args(&argv),
        vec!["--headless=new"],
        "a run without --debug must be headless-new exactly once; argv was {:?}",
        argv
    );
    assert!(
        !argv.iter().any(|a| a.starts_with("----")),
        "a switch was passed as a whole string and got a doubled prefix: {:?}",
        argv
    );
}

// ---------------------------------------------------------------------------
// #45: pages are closed, so a batch does not accumulate a tab per target.
// ---------------------------------------------------------------------------

/// Where the tests below find a browser, or `None`.
///
/// Deliberately *not* `resolve_chrome`: that downloads Chrome for Testing when
/// it finds none, and a unit test must not reach the network.
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

/// A target that navigates to a `data:` URL and reads its title, so the fetch
/// pipeline can be driven end to end without a network or a fixture server.
struct DataUrlTarget {
    id: usize,
}

impl FetchTarget for DataUrlTarget {
    type Output = String;
    type Accumulator = Option<String>;

    fn cache_key(&self) -> CacheKey {
        CacheKey::Product {
            country: "us".to_string(),
            product_id: format!("test-{}", self.id),
        }
    }

    fn url(&self, _page_num: usize) -> String {
        format!("data:text/html,<title>target-{}</title>", self.id)
    }

    fn navigation_context(&self) -> String {
        "Failed to navigate to the test page".to_string()
    }

    async fn extract(
        &self,
        _page: &chromiumoxide::Page,
        html: &str,
        acc: &mut Option<String>,
    ) -> anyhow::Result<Paging> {
        *acc = Some(html.to_string());
        Ok(Paging::Done)
    }

    fn finish(&self, acc: Option<String>) -> anyhow::Result<String> {
        acc.ok_or_else(|| anyhow::anyhow!("nothing was extracted"))
    }

    fn validate(&self, out: &String) -> anyhow::Result<()> {
        anyhow::ensure!(
            out.contains(&format!("target-{}", self.id)),
            "navigated to the wrong document"
        );
        Ok(())
    }
}

/// A target that navigates fine and then fails in `validate`, so the error path
/// through `fetch_on` is exercised by something other than inspection.
struct FailingTarget;

impl FetchTarget for FailingTarget {
    type Output = String;
    type Accumulator = Option<String>;

    fn cache_key(&self) -> CacheKey {
        CacheKey::Product {
            country: "us".to_string(),
            product_id: "test-failing".to_string(),
        }
    }

    fn url(&self, _page_num: usize) -> String {
        "data:text/html,<title>failing</title>".to_string()
    }

    fn navigation_context(&self) -> String {
        "Failed to navigate to the failing test page".to_string()
    }

    async fn extract(
        &self,
        _page: &chromiumoxide::Page,
        html: &str,
        acc: &mut Option<String>,
    ) -> anyhow::Result<Paging> {
        *acc = Some(html.to_string());
        Ok(Paging::Done)
    }

    fn finish(&self, acc: Option<String>) -> anyhow::Result<String> {
        acc.ok_or_else(|| anyhow::anyhow!("nothing was extracted"))
    }

    fn validate(&self, _out: &String) -> anyhow::Result<()> {
        anyhow::bail!("this target always rejects its own output")
    }
}

/// The tabs the browser has open once it has stopped closing any.
///
/// `Page::close` is a request, not an answer: the tab it names can still be
/// listed for a moment afterwards. Reading until two consecutive reads agree
/// asks "what is still open", which is the question #45 is about, rather than
/// "what was open at this instant", which is a race.
async fn settled_pages(session: &BrowserSession) -> Vec<String> {
    let mut previous = session.open_page_urls().await.expect("failed to list tabs");
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let current = session.open_page_urls().await.expect("failed to list tabs");
        if current == previous {
            return current;
        }
        previous = current;
    }
    previous
}

/// #45. `fetch_on` used to leak the page it opened. One process exit hid it;
/// #10's batch mode — N targets over one shared session — would not have.
///
/// The measurement is the one the issue asks for: the browser's own tab list,
/// read back over CDP after each fetch, against a growing number of targets.
/// Six fetches, five of which succeed and one of which fails after navigating,
/// because the error path leaked the same page the success path did.
#[tokio::test]
async fn a_batch_does_not_accumulate_a_tab_per_target() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let Some(chrome) = system_chrome() else {
        eprintln!("SKIPPED: no system Chrome; this test needs a real browser");
        return;
    };

    let scratch = Scratch::new("tabs");
    let config = test_config(scratch.path(), false);
    let session = BrowserSession::launch(chrome, &config)
        .await
        .expect("failed to launch the browser");

    let mut open_after_each = Vec::new();

    for id in 0..5 {
        let fetched = fetch_on(&DataUrlTarget { id }, &config, &session)
            .await
            .expect("the test target should fetch");
        assert!(
            fetched.data.contains(&format!("target-{}", id)),
            "the target navigated somewhere else entirely"
        );
        open_after_each.push(settled_pages(&session).await);
    }

    let failed = fetch_on(&FailingTarget, &config, &session).await;
    assert!(
        failed.is_err(),
        "FailingTarget was supposed to fail, so this measures the success path twice"
    );
    open_after_each.push(settled_pages(&session).await);

    let _ = session.close().await;

    // Every fetch navigated somewhere recognisable, so a page of ours that is
    // still listed is a page that was never closed.
    let leaked: Vec<&String> = open_after_each
        .last()
        .expect("six fetches were made")
        .iter()
        .filter(|url| url.starts_with("data:text/html"))
        .collect();
    assert!(
        leaked.is_empty(),
        "pages the fetch pipeline opened are still open after it returned: {:?}",
        leaked
    );

    // And the count is flat, not merely small: leaking one tab per target ends
    // at six more than it started with, which is the failure #10 would hit.
    let counts: Vec<usize> = open_after_each.iter().map(|p| p.len()).collect();
    assert!(
        counts.iter().all(|c| *c == counts[0]),
        "tab count grew with target count: after each of six fetches {:?} (tabs: {:?})",
        counts,
        open_after_each
    );
}
