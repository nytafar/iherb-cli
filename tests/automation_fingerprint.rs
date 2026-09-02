//! #48: what a launched Chrome tells the page about itself.
//!
//! Two findings that only make sense together. `--enable-automation` was in
//! chromiumoxide's `DEFAULT_ARGS` and reached Chrome on every launch — verified
//! present in captured argv — and the stealth JS meant to compensate had never
//! run on a real page, because `evaluate()` targeted `about:blank` and the
//! cross-document `goto` discarded every property it defined.
//!
//! So neither half is assertable by reading the code, and both are asserted
//! here against something the process did: the argv a real executable was
//! handed, and `navigator.webdriver` read **on a loaded page** rather than on
//! `about:blank`. Reading it on `about:blank` is precisely the mistake that
//! made the old snippet look like it worked.
//!
//! One of #48's premises did not survive being measured. The issue states that
//! `navigator.webdriver` reads `true` on the real page; on Google Chrome
//! 152.0.7977.66, headless-new, the pre-#48 argument set reads `false`, because
//! `--disable-blink-features=AutomationControlled` was already there and that
//! switch alone decides it. Recorded in `src/browser/session.rs` beside the
//! code. It changes none of the work: the snippet was still a no-op, its
//! pattern is still detectable, and `--enable-automation` is a separate signal
//! from the page reading — which is why the two are separate tests here.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::Mutex;

use iherb_cli::browser::session::BrowserSession;
use iherb_cli::config::{AppConfig, CacheMode, ProfileChoice, DEFAULT_CACHE_TTL};

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

fn config(scratch: &Path) -> AppConfig {
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
        // A throwaway profile: this file is about what Chrome is told, not
        // about what it keeps.
        profile: ProfileChoice::Throwaway,
        cache_dir: scratch.join("cache"),
        data_dir: scratch.join("data"),
    }
}

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

/// The argv a real `Browser::launch` handed the executable.
///
/// The recorder is not a mock of the builder: it is a program that
/// `Browser::launch` really execs, so what it writes down is the exact argv
/// Chrome would have received. That is the ground truth #48 needed and the
/// builder call could not give — `--enable-automation` was never written
/// anywhere in this repository, and it reached Chrome anyway.
async fn captured_argv(scratch: &Path) -> Vec<String> {
    let (exe, argv_file) = argv_recorder(scratch);
    let _ = BrowserSession::launch(exe, &config(scratch)).await;
    let recorded = std::fs::read_to_string(&argv_file).unwrap_or_else(|e| {
        panic!(
            "the launch never reached the executable, so no argv was captured: {}",
            e
        )
    });
    recorded.lines().map(|l| l.to_string()).collect()
}

/// The launch really happened and this recording really is its.
///
/// Without it, "no `--enable-automation` in the argv" would pass just as
/// happily on an empty file, which is the shape of regression test this
/// programme keeps shipping by accident.
fn assert_argv_is_ours(argv: &[String]) {
    assert!(
        argv.iter()
            .any(|a| a.starts_with("--user-data-dir=") && a.contains("iherb-cli-")),
        "captured argv is not from a BrowserSession launch: {:?}",
        argv
    );
    assert!(
        argv.iter().any(|a| a.starts_with("--user-agent=")),
        "captured argv is missing the user agent BrowserSession sets: {:?}",
        argv
    );
}

/// **`--enable-automation` does not reach Chrome.**
///
/// It is the loudest bot signal a Chrome instance can emit, it was never
/// written in this repository, and it was in the argv of every launch because
/// chromiumoxide passes puppeteer's `DEFAULT_ARGS` unless told not to.
#[tokio::test]
async fn enable_automation_is_not_in_the_argv() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let scratch = Scratch::new("automation");
    let argv = captured_argv(scratch.path()).await;

    assert_argv_is_ours(&argv);
    assert!(
        !argv.iter().any(|a| a == "--enable-automation"),
        "--enable-automation reached Chrome: {:?}",
        argv
    );
}

/// **Dropping it did not drop the 23 defaults that came with it.**
///
/// `disable_default_args()` removes the whole list, so the fix is only correct
/// if the rest is put back. Without this assertion the previous test passes on
/// a launch that also lost `--disable-background-networking`,
/// `--no-first-run` and twenty others — a much bigger behaviour change than the
/// one intended, and an invisible one.
///
/// Two of these, `--disable-hang-monitor` and
/// `--disable-ipc-flooding-protection`, this tool never set for itself; they
/// were coming from the default list, which is why the list is re-supplied
/// whole rather than filtered down to what looked useful.
#[tokio::test]
async fn the_rest_of_chromes_defaults_are_re_supplied() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let scratch = Scratch::new("defaults");
    let argv = captured_argv(scratch.path()).await;

    assert_argv_is_ours(&argv);

    let bare = [
        "--disable-background-networking",
        "--disable-background-timer-throttling",
        "--disable-backgrounding-occluded-windows",
        "--disable-breakpad",
        "--disable-client-side-phishing-detection",
        "--disable-component-extensions-with-background-pages",
        "--disable-default-apps",
        "--disable-dev-shm-usage",
        "--disable-hang-monitor",
        "--disable-ipc-flooding-protection",
        "--disable-popup-blocking",
        "--disable-prompt-on-repost",
        "--disable-renderer-backgrounding",
        "--disable-sync",
        "--metrics-recording-only",
        "--no-first-run",
        "--use-mock-keychain",
    ];
    for flag in bare {
        assert!(
            argv.iter().any(|a| a == flag),
            "{} was dropped along with --enable-automation: {:?}",
            flag,
            argv
        );
    }

    let valued = [
        "--force-color-profile=srgb",
        "--password-store=basic",
        "--enable-blink-features=IdleDetection",
        "--lang=en_US",
    ];
    for flag in valued {
        assert!(
            argv.iter().any(|a| a == flag),
            "{} was dropped along with --enable-automation: {:?}",
            flag,
            argv
        );
    }

    // Merged switches: chromiumoxide's argument builder is keyed by switch
    // name, so these arrive once each carrying every value asked for.
    let features = |key: &str| -> Vec<String> {
        argv.iter()
            .filter(|a| a.starts_with(&format!("--{}=", key)))
            .cloned()
            .collect()
    };
    let enabled = features("enable-features");
    assert_eq!(
        enabled.len(),
        1,
        "enable-features should arrive once, and arrived as {:?}",
        enabled
    );
    assert!(
        enabled[0].contains("NetworkService"),
        "the network service defaults were dropped: {:?}",
        enabled
    );

    let disabled = features("disable-features");
    assert_eq!(
        disabled.len(),
        1,
        "disable-features should arrive once, and arrived as {:?}",
        disabled
    );
    for value in ["TranslateUI", "IsolateOrigins", "site-per-process"] {
        assert!(
            disabled[0].contains(value),
            "{} is missing from the merged disable-features switch: {:?}",
            value,
            disabled
        );
    }
}

/// **`.hide()` supplies the blink switch this code used to spell by hand.**
///
/// The switch is the same; what changed is who owns it. Asserted so that
/// swapping the hand-written arg for the builder method cannot quietly drop it.
#[tokio::test]
async fn the_automation_controlled_blink_feature_is_still_disabled() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let scratch = Scratch::new("blink");
    let argv = captured_argv(scratch.path()).await;

    assert_argv_is_ours(&argv);
    assert!(
        argv.iter()
            .any(|a| a == "--disable-blink-features=AutomationControlled"),
        "the AutomationControlled blink feature is no longer disabled: {:?}",
        argv
    );
}

// ---------------------------------------------------------------------------
// On a real, loaded page.
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

/// A loopback origin serving one page.
///
/// A real document, because that is the whole point: the old snippet passed
/// every inspection anyone made of it on `about:blank` and had never survived a
/// navigation.
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
                .expect("non-blocking listener");
            loop {
                if rx.try_recv().is_ok() {
                    return;
                }
                match listener.accept() {
                    Ok((stream, _)) => serve(stream),
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

fn serve(mut stream: TcpStream) {
    let mut buf = [0u8; 1024];
    let _ = stream.read(&mut buf);
    let body = "<html><head><title>fingerprint</title></head><body>ok</body></html>";
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

/// **`navigator.webdriver` reads `false` on a page that has actually loaded.**
///
/// Three separate claims, and the second and third are what #48 is really
/// about:
///
///  - the value is `false`, not `true`, so the automation flag is hidden;
///  - it is not `undefined`, which is not a value any real browser reports and
///    is what the old snippet aimed for;
///  - `webdriver` is **not** an own property of `navigator`, because defining
///    it on the instance puts it in `Object.getOwnPropertyNames(navigator)`
///    where a real browser has nothing — a signal upstream documents as being
///    detectable in itself.
///
/// Read after a navigation, never on `about:blank`. `about:blank` is where the
/// old code looked correct.
#[tokio::test]
async fn navigator_webdriver_reads_false_on_a_loaded_page() {
    let _serial = ONE_AT_A_TIME.lock().await;
    let Some(chrome) = system_chrome() else {
        eprintln!("SKIPPED: no system Chrome; this test needs a real browser");
        return;
    };

    let origin = Origin::start();
    let scratch = Scratch::new("webdriver");
    let session = BrowserSession::launch(chrome, &config(scratch.path()))
        .await
        .expect("failed to launch the browser");
    let page = session.new_page().await.expect("failed to open a page");

    page.goto(&origin.url).await.expect("navigation failed");

    let readings: (String, bool, String) = page
        .evaluate(
            "[String(navigator.webdriver),
              Object.getOwnPropertyNames(navigator).includes('webdriver'),
              document.title]",
        )
        .await
        .expect("the page should answer")
        .into_value()
        .expect("three readings");

    let _ = session.close().await;

    let (webdriver, own_property, title) = readings;

    assert_eq!(
        title, "fingerprint",
        "the readings were taken somewhere other than the loaded page, which is \
         the mistake that made the old snippet look like it worked"
    );
    assert_eq!(
        webdriver, "false",
        "navigator.webdriver must read false on a real page. Removing `.hide()` \
         from the launch makes this read 'true', which is what pins the claim to \
         the switch that carries it; 'undefined' would mean the old \
         instance-level pattern is back, and no real browser reports that."
    );
    assert!(
        !own_property,
        "webdriver is an own property of navigator, so it was defined on the \
         instance rather than on the prototype — which is itself the detection \
         signal #48 is about"
    );
}
