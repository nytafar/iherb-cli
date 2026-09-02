//! #12: a profile directory survives the run, so what the browser learned in it
//! survives too.
//!
//! # What is asserted, and what is deliberately not
//!
//! The issue's acceptance criterion is "no Cloudflare interstitial on the
//! second run". **That is not verifiable here and a test asserting it would
//! pass for the wrong reason.** This programme has never received a live
//! Cloudflare challenge — 28 searches and 12 captures, none of them
//! interstitial — so clearance is *unmeasured*, not confirmed, and "no
//! interstitial appeared" is what a run against a site that never challenges
//! looks like whether or not any profile was reused.
//!
//! So the tests assert the mechanism the feature actually claims, which is
//! checkable today:
//!
//!  1. the second run launches Chrome against **the same directory** as the
//!     first, and the directory is **still there** afterwards;
//!  2. a cookie written during the first run is **readable by the second**,
//!     which is the property Cloudflare clearance would ride on.
//!
//! (2) needs a real browser and a real origin, so it serves its own page over
//! loopback — a `data:` or `about:blank` URL cannot hold a cookie — and skips
//! loudly when there is no system Chrome, rather than downloading one:
//! `cargo test` must not reach the network.
//!
//! (1) needs neither, because the argv a launch hands the executable is the
//! ground truth for which directory Chrome was pointed at, exactly as it is for
//! `--headless` in `tests/browser_lifecycle.rs`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use tokio::sync::Mutex;

use iherb_cli::browser::session::BrowserSession;
use iherb_cli::config::{AppConfig, CacheMode, ProfileChoice, DEFAULT_CACHE_TTL};

/// Chrome refuses to run two instances against one profile, so these tests are
/// serial among themselves as well as against the rest of the suite's launches.
static ONE_AT_A_TIME: Mutex<()> = Mutex::const_new(());

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

fn config(scratch: &Path, profile: ProfileChoice) -> AppConfig {
    AppConfig {
        country: "us".to_string(),
        currency: None,
        cache_mode: CacheMode::Off,
        cache_ttl: DEFAULT_CACHE_TTL,
        delay_ms: 0,
        debug: false,
        headful: false,
        timing: false,
        browser_path: None,
        profile,
        cache_dir: scratch.join("cache"),
        data_dir: scratch.join("data"),
    }
}

/// Write an executable that records the argv it was handed and exits, so a
/// launch can be observed without a browser.
///
/// `Browser::launch` fails against it — it never prints a DevTools URL — but it
/// fails *after* the exec, which is the only part these tests ask about.
fn argv_recorder(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
    let exe = dir.join(format!("{}.sh", name));
    let argv_file = dir.join(format!("{}-argv.txt", name));
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

/// The `--user-data-dir=` Chrome was handed on this launch.
fn user_data_dir(argv: &[String]) -> String {
    argv.iter()
        .find_map(|a| a.strip_prefix("--user-data-dir="))
        .unwrap_or_else(|| panic!("no --user-data-dir in the captured argv: {:?}", argv))
        .to_string()
}

/// Launch against the recorder and return the argv Chrome would have received.
async fn captured_argv(exe: PathBuf, argv_file: &Path, config: &AppConfig) -> Vec<String> {
    let _ = BrowserSession::launch(exe, config).await;
    let recorded = std::fs::read_to_string(argv_file).unwrap_or_else(|e| {
        panic!(
            "the launch never reached the executable, so no argv was captured: {}",
            e
        )
    });
    recorded.lines().map(|l| l.to_string()).collect()
}

/// **The directory is reused, and it is still there afterwards.**
///
/// Two launches against one `--profile-dir`, and the assertion is on the argv
/// each one handed the executable: the same path both times, that path and not
/// a temp directory, and the directory still on disk when both are done.
///
/// Before #12 both runs got `/tmp/iherb-cli-<pid>-<millis>` — a different
/// directory each time, deleted on the way out — so cookies, storefront
/// preferences and any Cloudflare clearance died with each run. The two
/// assertions are separable on purpose: reusing a path that has been emptied
/// would satisfy the first and not the second, and it is the second that the
/// feature is for.
#[tokio::test]
async fn a_named_profile_dir_is_reused_and_never_deleted() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let scratch = Scratch::new("profile-reuse");
    let profile = scratch.path().join("chrome-profile");
    let config = config(scratch.path(), ProfileChoice::Stated(profile.clone()));

    let (exe, argv_file) = argv_recorder(scratch.path(), "first");
    let first = captured_argv(exe, &argv_file, &config).await;

    assert!(
        profile.exists(),
        "the first run must leave the profile directory it was given: {}",
        profile.display()
    );

    let (exe, argv_file) = argv_recorder(scratch.path(), "second");
    let second = captured_argv(exe, &argv_file, &config).await;

    assert_eq!(
        user_data_dir(&first),
        profile.display().to_string(),
        "the first run was pointed somewhere other than the directory it was given"
    );
    assert_eq!(
        user_data_dir(&second),
        user_data_dir(&first),
        "the second run must reuse the first run's profile directory; a fresh \
         temp directory per run is exactly what #12 removes"
    );
    assert!(
        profile.exists(),
        "a profile directory the caller named must never be deleted by this \
         tool, and {} is gone",
        profile.display()
    );
}

/// **`--no-profile` still gets a throwaway directory, and still removes it.**
///
/// The pre-#12 behaviour has to remain reachable, and the removal has to remain
/// attached to it: a fix that made every profile persistent would leak a
/// directory per run into the temp directory, which is what #46 was about.
#[tokio::test]
async fn no_profile_still_gets_a_temp_directory_and_removes_it() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let scratch = Scratch::new("profile-throwaway");
    let config = config(scratch.path(), ProfileChoice::Throwaway);

    let (exe, argv_file) = argv_recorder(scratch.path(), "throwaway");
    let argv = captured_argv(exe, &argv_file, &config).await;

    let dir = user_data_dir(&argv);
    assert!(
        dir.starts_with(&std::env::temp_dir().display().to_string()) && dir.contains("iherb-cli-"),
        "--no-profile must use a throwaway directory under the temp dir, and got {}",
        dir
    );
    assert!(
        !Path::new(&dir).exists(),
        "a throwaway profile must be gone once the session is, and {} is still there",
        dir
    );
}

/// **The default profile is persistent and lives under the data directory.**
///
/// #12 asks for the benefit to be on without a flag. Asserted on the argv and
/// on the filesystem rather than on `ProfileChoice::default_dir`, which is the
/// function under test.
#[tokio::test]
async fn the_default_profile_is_persistent_under_the_data_dir() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let scratch = Scratch::new("profile-default");
    let config = config(scratch.path(), ProfileChoice::Default);

    let (exe, argv_file) = argv_recorder(scratch.path(), "default");
    let argv = captured_argv(exe, &argv_file, &config).await;

    let dir = PathBuf::from(user_data_dir(&argv));
    assert!(
        dir.starts_with(&config.data_dir),
        "the default profile belongs under the data directory {}, and was {}",
        config.data_dir.display(),
        dir.display()
    );
    assert!(
        dir.exists(),
        "the default profile must survive the run that created it, and {} is gone",
        dir.display()
    );
}

/// **A second run against a profile the caller named fails loudly.**
///
/// Chrome answers a shared profile with a `SingletonLock` failure that says
/// nothing about who is holding it. #12 asks for the concurrent case to fail
/// loudly or degrade gracefully rather than mysteriously, and #55 settles which
/// of the two applies where: a directory the caller *named* binds, so the
/// second run refuses instead of quietly running somewhere else.
///
/// The lock is held by a live session, so this also asserts the lock is held
/// for the session's lifetime rather than only across its creation.
#[tokio::test]
async fn two_runs_cannot_share_a_named_profile_dir() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let Some(chrome) = system_chrome() else {
        eprintln!("SKIPPED: no system Chrome; holding a profile needs a real session");
        return;
    };

    let scratch = Scratch::new("profile-contended");
    let profile = scratch.path().join("shared");
    let config = config(scratch.path(), ProfileChoice::Stated(profile.clone()));

    let held = BrowserSession::launch(chrome.clone(), &config)
        .await
        .expect("the first run should get the profile");

    let error = match BrowserSession::launch(chrome, &config).await {
        Ok(_) => panic!("two runs must not both hold one named profile directory"),
        Err(e) => e,
    };
    assert!(
        error.to_string().contains(&profile.display().to_string()),
        "the refusal has to name the directory that is in use, and said: {}",
        error
    );

    let _ = held.close().await;
    assert!(
        profile.exists(),
        "the contended run must not remove a directory it never owned"
    );
}

/// **The default profile degrades to a throwaway one rather than failing.**
///
/// The other half of the rule. Nobody named the default directory, so a second
/// concurrent run is not breaking a stated constraint by using somewhere else —
/// and failing every second run of a tool whose profile is on by default would
/// be worse than the cold browser it replaced. It is warned about, because the
/// run that degrades is the run whose clearance will not persist.
#[tokio::test]
async fn a_contended_default_profile_degrades_instead_of_failing() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let Some(chrome) = system_chrome() else {
        eprintln!("SKIPPED: no system Chrome; holding a profile needs a real session");
        return;
    };

    let scratch = Scratch::new("profile-degrade");
    let config = config(scratch.path(), ProfileChoice::Default);

    let held = BrowserSession::launch(chrome.clone(), &config)
        .await
        .expect("the first run should get the default profile");
    assert!(
        !held.profile_is_temporary(),
        "the first run holds the default profile, which is persistent"
    );

    let second = BrowserSession::launch(chrome, &config)
        .await
        .expect("a second run must not fail on a profile nobody named");
    assert!(
        second.profile_is_temporary(),
        "the second run has to fall back to a throwaway profile, and instead \
         reports it owns the persistent one at {}",
        second.profile_dir().display()
    );
    assert_ne!(
        second.profile_dir(),
        held.profile_dir(),
        "two live sessions must not be writing into one profile directory"
    );

    let fallback = second.profile_dir().to_path_buf();
    let _ = second.close().await;
    let _ = held.close().await;
    assert!(
        !fallback.exists(),
        "the throwaway fallback must still clean itself up, and {} is still there",
        fallback.display()
    );
}

// ---------------------------------------------------------------------------
// The cookie assertion: a real browser, a real origin, two runs.
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

/// A loopback origin serving one trivial page, for as long as it is held.
///
/// A cookie needs an origin: CDP refuses to set one against `about:blank` or a
/// `data:` URL, which is what every other browser test in this repo navigates
/// to. Fourteen lines of `TcpListener` rather than a web-server dependency,
/// because the page's content is irrelevant — what is under test is the cookie
/// jar, not the HTML.
struct Origin {
    url: String,
    shutdown: Option<std::sync::mpsc::Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Origin {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel::<()>();

        let handle = std::thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("the listener must not block the shutdown check");
            loop {
                if rx.try_recv().is_ok() {
                    return;
                }
                match listener.accept() {
                    Ok((stream, _)) => serve(stream),
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
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

fn serve(mut stream: TcpStream) {
    let mut buf = [0u8; 1024];
    let _ = stream.read(&mut buf);
    let body = "<html><title>profile</title><body>ok</body></html>";
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

/// The cookie named `name` that this page can see, if any.
async fn cookie_value(page: &chromiumoxide::Page, name: &str) -> Option<String> {
    page.get_cookies()
        .await
        .expect("the browser should answer a cookie query")
        .into_iter()
        .find(|c| c.name == name)
        .map(|c| c.value)
}

/// **A cookie written on the first run is there on the second.**
///
/// This is #12's real claim, stated as something a test can decide. Cloudflare
/// clearance is a cookie; so is a storefront preference. If a cookie written
/// into a named profile is readable by a later run against the same profile,
/// clearance would survive the same way — and if it is not, no amount of
/// "no interstitial appeared" would mean the feature worked.
///
/// The cookie is given an expiry a year out, because a session cookie is never
/// written to disk and would fail this test for a reason that has nothing to do
/// with the profile. The first session is closed gracefully rather than dropped,
/// because flushing the cookie jar is part of what closing a browser does.
#[tokio::test]
async fn cookies_written_on_the_first_run_are_present_on_the_second() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let Some(chrome) = system_chrome() else {
        eprintln!("SKIPPED: no system Chrome; this test needs a real browser");
        return;
    };

    let origin = Origin::start();
    let scratch = Scratch::new("profile-cookies");
    let profile = scratch.path().join("chrome-profile");
    let config = config(scratch.path(), ProfileChoice::Stated(profile.clone()));

    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        + 365.0 * 24.0 * 60.0 * 60.0;

    // Run one: write the cookie, then close the browser properly so the jar is
    // flushed to the profile on disk.
    {
        let session = BrowserSession::launch(chrome.clone(), &config)
            .await
            .expect("the first run should launch");
        assert!(
            !session.profile_is_temporary(),
            "a named profile is not this tool's to delete, and the session says it is"
        );

        let page = session.new_page().await.expect("first page");
        page.goto(&origin.url).await.expect("first navigation");

        let mut cookie = chromiumoxide::cdp::browser_protocol::network::CookieParam::new(
            "iherb_cli_profile_probe",
            "carried-over",
        );
        cookie.url = Some(origin.url.clone());
        cookie.path = Some("/".to_string());
        cookie.expires =
            Some(chromiumoxide::cdp::browser_protocol::network::TimeSinceEpoch::new(expires));
        page.set_cookie(cookie).await.expect("set the probe cookie");

        assert_eq!(
            cookie_value(&page, "iherb_cli_profile_probe")
                .await
                .as_deref(),
            Some("carried-over"),
            "the test's own premise is broken: the cookie was not set at all"
        );

        session.close().await.expect("the first run should close");
    }

    // Run two: the same directory, a brand-new browser process.
    let session = BrowserSession::launch(chrome, &config)
        .await
        .expect("the second run should launch");
    let page = session.new_page().await.expect("second page");
    page.goto(&origin.url).await.expect("second navigation");

    let carried = cookie_value(&page, "iherb_cli_profile_probe").await;
    let _ = session.close().await;

    assert_eq!(
        carried.as_deref(),
        Some("carried-over"),
        "a cookie written into {} on the first run was not there on the second, \
         so nothing a run learns in the browser survives it — which is the whole \
         of #12",
        profile.display()
    );
}

/// **`--profile-dir` and `--no-profile` together are refused.**
///
/// A contradiction rather than a redundancy: there is no reading of "keep the
/// profile at this path" and "keep no profile" that honours both. `--no-cache`
/// and `--refresh` are ordered by strength instead, because one of them *is*
/// the stronger version of the other; these two are not.
///
/// Asserted through `AppConfig::load`, which is the path a real command line
/// takes, rather than through `ProfileChoice::from_flags` alone — a flag pair
/// that never reached the resolver would pass that.
#[test]
fn the_two_profile_flags_cannot_both_be_given() {
    let error = iherb_cli::config::AppConfig::load(&iherb_cli::cli::GlobalArgs {
        profile_dir: Some(PathBuf::from("/tmp/somewhere")),
        no_profile: true,
        ..iherb_cli::cli::GlobalArgs::none()
    })
    .expect_err("--profile-dir with --no-profile asks for opposite things");

    assert_eq!(
        error.kind(),
        iherb_cli::error::ErrorKind::InvalidInput,
        "a contradictory pair of flags is the caller's input; got: {}",
        error
    );
    assert!(
        error.to_string().contains("--no-profile"),
        "the refusal has to name both flags so the caller knows which to drop, \
         and said: {}",
        error
    );
}

/// **Each flag on its own resolves to the choice it names**, and no flags at
/// all resolves to the persistent default.
///
/// The default is the assertion that matters: #12 asks for the benefit to be on
/// without a flag, so a resolution that quietly kept `Throwaway` as the default
/// would leave every ordinary run exactly as cold as before.
#[test]
fn the_profile_flags_resolve_as_documented() {
    let named = PathBuf::from("/tmp/a-profile");

    let cases = [
        (None, false, ProfileChoice::Default),
        (None, true, ProfileChoice::Throwaway),
        (
            Some(named.clone()),
            false,
            ProfileChoice::Stated(named.clone()),
        ),
    ];

    for (profile_dir, no_profile, expected) in cases {
        let config = iherb_cli::config::AppConfig::load(&iherb_cli::cli::GlobalArgs {
            profile_dir: profile_dir.clone(),
            no_profile,
            ..iherb_cli::cli::GlobalArgs::none()
        })
        .expect("the flags should resolve");
        assert_eq!(
            config.profile, expected,
            "--profile-dir {:?} with --no-profile {} resolved wrongly",
            profile_dir, no_profile
        );
    }
}
