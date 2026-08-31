//! What the browser actually does, observed rather than read.
//!
//! The bugs this file guards were invisible to code review: the builder call
//! *said* `("headless", "new")` and it was not what Chrome received. So every
//! assertion here is made against something the process did — an argv vector a
//! real executable was handed — and never against the call that was supposed to
//! produce it.
//!
//! `--headless` is asserted through a stub executable rather than through
//! Chrome itself. The stub is not a mock of the builder: it is a real program
//! that `Browser::launch` really execs, so what it writes down is the exact
//! argv vector Chrome would have received. That is the ground truth #47 needed
//! and the builder call could not give.

use std::path::{Path, PathBuf};

use iherb_cli::browser::session::BrowserSession;
use iherb_cli::config::AppConfig;

/// A config that touches nothing on disk that matters: caching off, no delay,
/// cache and data directories under a temp path of the test's own.
fn test_config(scratch: &Path, debug: bool) -> AppConfig {
    AppConfig {
        country: "us".to_string(),
        currency: "USD".to_string(),
        no_cache: true,
        delay_ms: 0,
        debug,
        browser_path: None,
        cache_dir: scratch.join("cache"),
        data_dir: scratch.join("data"),
    }
}

fn scratch_dir(name: &str) -> PathBuf {
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
    dir
}

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

    // A launch that never got as far as exec would leave the profile directory
    // behind; sweep whatever this call created either way, so the assertions
    // below are the only thing that can fail.
    for dir in profile_dirs() {
        if !before.contains(&dir) {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

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
    let scratch = scratch_dir("headful");
    let argv = captured_argv(&scratch, true).await;
    let _ = std::fs::remove_dir_all(&scratch);

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
    let scratch = scratch_dir("headless");
    let argv = captured_argv(&scratch, false).await;
    let _ = std::fs::remove_dir_all(&scratch);

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
