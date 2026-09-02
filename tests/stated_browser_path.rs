//! #55: a browser path the caller stated is the browser that runs, or the run
//! fails.
//!
//! `iherb-cli product 12949 --browser-path /nonexistent --no-cache --json`
//! exited **0** with `ok: true` and a full product record, produced by system
//! Chrome. `resolve_chrome` warned to stderr and fell through.
//!
//! Every assertion here goes through the production resolver, and the two that
//! matter most go through `get_or_launch_browser` — the function the fetch
//! pipeline actually calls — so what is under test is the chain a real run
//! takes, including the `.context(..)` wrapper that `classify_error` has to see
//! past. A test that called `resolve_chrome` alone would pass even if the
//! error were reclassified into `internal_error` one layer up.
//!
//! Nothing here launches a browser or reaches the network: every case is
//! decided before the executable is touched, which is precisely the claim.

use std::path::PathBuf;

use iherb_cli::browser::resolve::resolve_chrome;
use iherb_cli::config::{
    AppConfig, BrowserPathSource, CacheMode, ProfileChoice, StatedBrowserPath, DEFAULT_CACHE_TTL,
};
use iherb_cli::error::{classify_error, ErrorKind};

/// A scratch directory that removes itself, so a failing assertion does not
/// leave litter for the next run to read past.
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

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config_with(scratch: &std::path::Path, browser_path: Option<StatedBrowserPath>) -> AppConfig {
    AppConfig {
        country: "us".to_string(),
        currency: None,
        cache_mode: CacheMode::Off,
        cache_ttl: DEFAULT_CACHE_TTL,
        delay_ms: 0,
        debug: false,
        headful: false,
        browser_path,
        profile: ProfileChoice::Throwaway,
        cache_dir: scratch.join("cache"),
        data_dir: scratch.join("data"),
    }
}

/// A path that is guaranteed not to exist, under a directory that does.
fn missing_under(scratch: &std::path::Path, name: &str) -> PathBuf {
    let path = scratch.join(name);
    assert!(
        !path.exists(),
        "the test's own premise is broken: {} exists",
        path.display()
    );
    path
}

/// **The flag binds.** A `--browser-path` that does not exist fails the run.
///
/// The assertion is on the taxonomy rather than on the string: `invalid_input`
/// and exit `2`, and the message naming both the path that was tried and the
/// flag that named it. A message alone would pass on a run that reported the
/// right prose under `internal_error`, which is the classification that tells a
/// caller to file a bug about its own typo.
#[tokio::test]
async fn a_stated_browser_path_that_does_not_exist_fails_the_run() {
    let scratch = Scratch::new("stated-flag");
    let missing = missing_under(scratch.path(), "not-a-browser");

    let error = resolve_chrome(
        Some(&StatedBrowserPath::new(
            missing.clone(),
            BrowserPathSource::Flag,
        )),
        scratch.path(),
    )
    .await
    .expect_err("a browser path that does not exist must not resolve to some other browser");

    assert_eq!(
        error.kind(),
        ErrorKind::InvalidInput,
        "a path the caller named and that is not there is the caller's input, \
         not the environment; the message was: {}",
        error
    );
    assert_eq!(
        error.kind().exit_code(),
        2,
        "the run used to exit 0 with ok: true"
    );

    let message = error.to_string();
    assert!(
        message.contains(&missing.display().to_string()),
        "the error has to name the path that was tried, and said: {}",
        message
    );
    assert!(
        message.contains("--browser-path"),
        "the error has to name the source to correct, and said: {}",
        message
    );
}

/// **The environment variable and the config file bind the same way**, and the
/// error names which one to go and edit.
///
/// This is the half of #55 that asks for a *decision* rather than a side
/// effect of the three sources sharing a type. The decision is that they are
/// alike: a browser you named is the browser that runs. What differs is only
/// the sentence the caller is handed.
#[tokio::test]
async fn every_stated_source_binds_and_the_error_names_it() {
    let scratch = Scratch::new("stated-sources");
    let missing = missing_under(scratch.path(), "moved-chrome");
    let config_file = scratch.path().join("config.toml");

    let cases = [
        (BrowserPathSource::Env, "IHERB_BROWSER_PATH".to_string()),
        (
            BrowserPathSource::ConfigFile(config_file.clone()),
            format!("browser_path in {}", config_file.display()),
        ),
    ];

    for (source, expected) in cases {
        let error = resolve_chrome(
            Some(&StatedBrowserPath::new(missing.clone(), source.clone())),
            scratch.path(),
        )
        .await
        .expect_err("a stated path that does not exist must fail whichever source stated it");

        assert_eq!(
            error.kind(),
            ErrorKind::InvalidInput,
            "{:?} must fail the same way the flag does; the message was: {}",
            source,
            error
        );
        assert!(
            error.to_string().contains(&expected),
            "the error for {:?} has to name '{}' so the caller knows where to \
             correct it, and said: {}",
            source,
            expected,
            error
        );
    }
}

/// **The fetch pipeline reports it as `invalid_input`, exit 2.**
///
/// `get_or_launch_browser` wraps the resolver in `.context("Failed to resolve
/// Chrome browser")`, and `classify_error` walks the chain rather than reading
/// the outermost error. Asserting on the resolver alone would not notice a
/// context wrapper that buried the typed error, which would report
/// `internal_error` (70) — "file a bug about this tool" — for a corrigible
/// typo.
///
/// The browser is never launched: the failure happens before a process starts,
/// which is exactly why this is the caller's input and not the environment.
#[tokio::test]
async fn the_pipeline_reports_a_bad_stated_path_as_invalid_input() {
    let scratch = Scratch::new("stated-pipeline");
    let missing = missing_under(scratch.path(), "no-such-chrome");
    let config = config_with(
        scratch.path(),
        Some(StatedBrowserPath::new(
            missing.clone(),
            BrowserPathSource::Flag,
        )),
    );

    let mut session = None;
    // Matched rather than `expect_err`: `BrowserSession` is not `Debug`, and a
    // success here would in any case mean a live browser this test would then
    // have to shut down.
    let error = match iherb_cli::fetch::get_or_launch_browser(&config, &mut session).await {
        Ok(_) => panic!("a run that names a browser that is not there must not launch one"),
        Err(e) => e,
    };

    assert!(
        session.is_none(),
        "no browser may be launched for a path that does not exist"
    );
    assert_eq!(
        classify_error(&error),
        ErrorKind::InvalidInput,
        "the pipeline has to report the caller's typo as the caller's typo; it \
         reported: {:#}",
        error
    );
    assert!(
        format!("{:#}", error).contains(&missing.display().to_string()),
        "the path that was tried has to survive the context wrapper: {:#}",
        error
    );
}

/// **Nothing else changed.** With no stated path, the resolver still falls
/// through, and that is deliberate rather than an oversight.
///
/// System detection, the previously downloaded Chrome and the auto-download are
/// this function's own answer to "find me a browser". Nobody stated them, so
/// there is no constraint to break by trying the next one. #55 removes silent
/// *substitution for something the caller said*, not fallback as such — and a
/// fix that made every path binding would turn a machine with no Chrome into a
/// hard failure instead of a download.
///
/// Asserted without reaching the network: on a machine with system Chrome the
/// resolver answers with it, and on one without, this test says so and stops
/// rather than downloading.
#[tokio::test]
async fn no_stated_path_still_falls_through_to_detection() {
    let scratch = Scratch::new("stated-none");
    let config = config_with(scratch.path(), None);

    let system_chrome = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.exists());

    let Some(_) = system_chrome else {
        eprintln!(
            "SKIPPED: no system Chrome, so the fall-through cannot be observed \
             without downloading one"
        );
        return;
    };

    let resolved = resolve_chrome(config.browser_path.as_ref(), &config.data_dir)
        .await
        .expect("with nothing stated, detection must still find the system browser");
    assert!(
        resolved.exists(),
        "detection answered with a path that is not there: {}",
        resolved.display()
    );
}
