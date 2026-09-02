use crate::config::AppConfig;
use crate::error::IherbError;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::Page;
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

const STEALTH_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

// chromiumoxide 0.9 parses switches into a key/value `Arg` rather than passing
// the string through verbatim: it writes the leading `--` itself and joins
// values with `,`. A whole switch handed over as one string therefore becomes
// the *key*, and Chrome receives `----no-first-run`, which it ignores in
// silence. So the switches are split here: bare flags without the `--`, and
// anything with a value as a `(key, value)` or `(key, values)` tuple.
const STEALTH_FLAGS: &[&str] = &[
    "disable-site-isolation-trials",
    "disable-web-security",
    "no-first-run",
    "no-default-browser-check",
    "disable-default-apps",
    "disable-extensions",
    "disable-popup-blocking",
    "disable-translate",
    "disable-background-timer-throttling",
    "disable-renderer-backgrounding",
    "disable-backgrounding-occluded-windows",
];

const STEALTH_DISABLED_FEATURES: &[&str] = &["IsolateOrigins", "site-per-process"];

/// How many times a profile directory removal is attempted before giving up.
const CLEANUP_ATTEMPTS: u32 = 4;

/// How long to wait between those attempts.
///
/// Chrome's child processes are killed by `kill_on_drop`, which chromiumoxide
/// documents as reaping them "in the background" with **no guarantee as to
/// when**. So a removal that fails usually means "not yet" rather than "never",
/// and the wait is what turns the first answer into the second.
const CLEANUP_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// A temporary profile directory that removes itself.
///
/// # Why a guard and not an error arm
///
/// [`BrowserSession::launch`] used to create the directory and remove it in the
/// `Err` arm of the launch it awaited. That covers a launch that *fails* and
/// not a launch that is *cancelled* — and cancellation is the case #46 is
/// about. Ctrl+C drops the command future while the launch is still in flight;
/// a dropped future runs no arm of the match it was suspended inside, so the
/// removal never happened, and `app.rs` could not clean up after it either
/// because no session had been assigned yet for it to find. The directory
/// stayed. A dropped future *does* run the destructors of its live locals,
/// which is the one hook cancellation cannot skip, so that is where the removal
/// now lives.
///
/// # There is no `disarm` flag
///
/// Ownership is the disarm. [`BrowserSession::launch_into`] takes this by value
/// and stores it in the session it returns, so the guard removes the directory
/// on exactly the paths where no session came into existence, and on none where
/// one did. A boolean would be a second thing to keep in step with the move
/// that already says it.
struct ProfileDir {
    path: PathBuf,
}

impl ProfileDir {
    /// Create a profile directory no other run can be using.
    ///
    /// The name carries the pid and a millisecond timestamp to avoid the
    /// `SingletonLock` conflict two concurrent runs would otherwise hit, and to
    /// avoid inheriting a stale lock left behind by a previous one.
    fn create() -> Result<Self, IherbError> {
        let path = std::env::temp_dir().join(format!(
            "iherb-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        ));
        std::fs::create_dir_all(&path).map_err(|e| {
            IherbError::BrowserLaunch(format!(
                "Failed to create user data dir {}: {}",
                path.display(),
                e
            ))
        })?;
        Ok(Self { path })
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for ProfileDir {
    fn drop(&mut self) {
        remove_profile_dir(&self.path);
    }
}

/// Remove a profile directory, giving Chrome time to let go of it first.
///
/// One implementation, reached from every path that can end a session: a
/// successful close, a close that failed, a panic unwinding past the session,
/// and an interrupt that drops it. It used to exist twice — a patient version
/// inside `close`, which a `?` could return past, and a single-shot bare
/// `remove_dir_all` in `Drop`, which is the one every non-happy path actually
/// got. The single-shot version is the one most likely to fail, because the
/// paths that reach it are the paths where Chrome is still dying.
///
/// Blocking rather than async on purpose: `Drop` cannot await, and a second
/// async copy for `close` to use would be a second thing to keep correct. The
/// wait is only paid when a removal actually fails.
fn remove_profile_dir(path: &std::path::Path) {
    for attempt in 1..=CLEANUP_ATTEMPTS {
        if !path.exists() {
            return;
        }
        match std::fs::remove_dir_all(path) {
            Ok(()) => return,
            Err(e) if attempt < CLEANUP_ATTEMPTS => {
                tracing::debug!(
                    "Cleanup attempt {}/{} for {}: {}, retrying...",
                    attempt,
                    CLEANUP_ATTEMPTS,
                    path.display(),
                    e
                );
                std::thread::sleep(CLEANUP_SETTLE);
            }
            Err(e) => {
                tracing::debug!(
                    "Could not clean up temp dir {}: {}. The OS will clean /tmp.",
                    path.display(),
                    e
                );
            }
        }
    }
}

pub struct BrowserSession {
    /// Declared before [`BrowserSession::profile`], and the order is
    /// load-bearing: struct fields drop in declaration order, so dropping a
    /// session kills Chrome *before* the profile directory Chrome was writing
    /// into is removed. The other order asks the filesystem to delete a tree a
    /// live browser is still adding files to, which is how a half-removed
    /// directory gets left behind.
    browser: Arc<Mutex<Browser>>,
    _handle: tokio::task::JoinHandle<()>,
    profile: ProfileDir,
}

impl BrowserSession {
    pub async fn launch(chrome_path: PathBuf, config: &AppConfig) -> Result<Self, IherbError> {
        // The directory is handed straight to the launch and never touched
        // again here. Nothing else can clean it up if the launch fails or is
        // cancelled — `close` belongs to a session that in those cases never
        // exists — so [`ProfileDir`] owns it from the moment it exists until a
        // session takes it over. Chrome that will not start is exactly when a
        // launch gets retried, so this is the leak that repeats (#46).
        Self::launch_into(chrome_path, config, ProfileDir::create()?).await
    }

    /// The launch proper, on a profile directory that already exists.
    ///
    /// `profile` is taken **by value**: this either returns a session that owns
    /// it, or does not return, and in the second case the guard goes out of
    /// scope and the directory goes with it. That covers the cancellation this
    /// function is suspended inside, which is what a match on its result could
    /// not (#46).
    async fn launch_into(
        chrome_path: PathBuf,
        config: &AppConfig,
        profile: ProfileDir,
    ) -> Result<Self, IherbError> {
        let mut builder = BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(profile.path())
            .arg(("user-agent", STEALTH_USER_AGENT))
            .arg(("disable-blink-features", "AutomationControlled"))
            .arg(("disable-features", STEALTH_DISABLED_FEATURES))
            .window_size(1920, 1080)
            .viewport(None);

        for flag in STEALTH_FLAGS {
            builder = builder.arg(*flag);
        }

        // Headless is a *mode* on the builder, not a switch in `args`, and the
        // mode defaults to `HeadlessMode::True` (#47). Setting `("headless",
        // "new")` as an ordinary arg therefore never produced a headful
        // browser: on the headful path nothing was set, the default mode
        // still appended `--headless`, and the flag only ever changed logging.
        // It looked right on the headless path purely by accident — the mode
        // appends a *valueless* `headless` key, which merges into the `new`
        // value already under that key and comes out as `--headless=new`.
        //
        // The window belongs to `--headful` alone (#62). It used to belong to
        // `--debug`, which meant the HTML dump — the cheapest diagnosis this
        // repo has — could not be taken without a display, so it was
        // unavailable in CI, over SSH and in an unattended run. What genuinely
        // wants a window is a human completing a Cloudflare challenge, and
        // that is what `--headful` is now for.
        builder = if config.headful {
            builder.with_head()
        } else {
            builder.new_headless_mode()
        };

        // Chrome refuses to run as root without --no-sandbox
        #[cfg(target_os = "linux")]
        if unsafe { libc::geteuid() } == 0 {
            builder = builder.arg("no-sandbox");
        }

        let browser_config = builder
            .build()
            .map_err(|e| IherbError::BrowserLaunch(e.to_string()))?;

        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| IherbError::BrowserLaunch(e.to_string()))?;

        let handle = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                tracing::trace!("Browser event: {:?}", event);
            }
        });

        Ok(BrowserSession {
            browser: Arc::new(Mutex::new(browser)),
            _handle: handle,
            profile,
        })
    }

    pub async fn new_page(&self) -> Result<Page, IherbError> {
        let browser = self.browser.lock().await;
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| IherbError::BrowserLaunch(format!("Failed to create page: {}", e)))?;

        // Stealth: override navigator.webdriver and other detection vectors
        let _ = page
            .evaluate(
                r#"
                Object.defineProperty(navigator, 'webdriver', { get: () => undefined });
                Object.defineProperty(navigator, 'languages', { get: () => ['en-US', 'en'] });
                Object.defineProperty(navigator, 'plugins', { get: () => [1, 2, 3, 4, 5] });

                // Override chrome.runtime to prevent detection
                window.chrome = { runtime: {} };

                // Override permissions query
                const originalQuery = window.navigator.permissions.query;
                window.navigator.permissions.query = (parameters) => (
                    parameters.name === 'notifications' ?
                    Promise.resolve({ state: Notification.permission }) :
                    originalQuery(parameters)
                );
                "#,
            )
            .await;

        Ok(page)
    }

    /// The temporary profile directory Chrome was launched against.
    ///
    /// Exposed so a test can watch the directory rather than take the code's
    /// word for it (#46). Callers no longer need it to finish a cleanup someone
    /// else abandoned: dropping the session is the cleanup, on every path.
    pub fn profile_dir(&self) -> &std::path::Path {
        self.profile.path()
    }

    /// The URL of every tab this browser currently has open.
    ///
    /// Read back over CDP rather than counted locally, so it is the browser's
    /// answer and not ours. It exists because #45 could not be demonstrated any
    /// other way: the fetch pipeline *said* it opened a page per target, and
    /// nothing said how many were still open afterwards. #10 will want the same
    /// answer once it runs targets concurrently.
    ///
    /// A tab Chrome is still tearing down can appear here for a moment after
    /// `Page::close` returns, because closing is a request rather than an
    /// answer. A caller that wants a settled count has to read twice.
    pub async fn open_page_urls(&self) -> Result<Vec<String>, IherbError> {
        let browser = self.browser.lock().await;
        let pages = browser
            .pages()
            .await
            .map_err(|e| IherbError::BrowserLaunch(format!("Failed to list pages: {}", e)))?;

        let mut urls = Vec::with_capacity(pages.len());
        for page in pages {
            urls.push(page.url().await.ok().flatten().unwrap_or_default());
        }
        Ok(urls)
    }

    /// Ask Chrome to shut down, and clean up after it either way.
    ///
    /// There is no `?` on the close. There used to be, and it returned past the
    /// only patient cleanup there was: a browser that would not close — the
    /// case where Chrome is *least* likely to have let go of its profile — fell
    /// through to a single-shot bare removal in `Drop` and left the directory
    /// behind (#46). The removal is not this function's job any more. It
    /// belongs to `self`, which drops on the way out of here whatever the close
    /// said, and drops the same way when a panic unwinds past a session or an
    /// interrupt abandons one.
    pub async fn close(self) -> Result<(), IherbError> {
        let closed =
            {
                let mut browser = self.browser.lock().await;
                browser.close().await.map(|_| ()).map_err(|e| {
                    IherbError::BrowserLaunch(format!("Failed to close browser: {}", e))
                })
            };

        // Explicit, because the ordering is the point: this kills Chrome and
        // then removes the profile directory, in that order, before the result
        // above is returned to a caller who might otherwise assume a failed
        // close means nothing was cleaned up.
        drop(self);

        closed
    }
}
