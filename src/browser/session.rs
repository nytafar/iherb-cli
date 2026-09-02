use crate::config::{AppConfig, ProfileChoice};
use crate::error::IherbError;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::Page;
use futures::StreamExt;
use std::fs::File;
use std::path::{Path, PathBuf};
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

/// How long a graceful browser close is given before it is killed (#12).
///
/// Three seconds, which is the fork's number and is long enough for a Chrome
/// that is going to answer. What it bounds is a Chrome that is not.
const CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// How long to wait between those attempts.
///
/// Chrome's child processes are killed by `kill_on_drop`, which chromiumoxide
/// documents as reaping them "in the background" with **no guarantee as to
/// when**. So a removal that fails usually means "not yet" rather than "never",
/// and the wait is what turns the first answer into the second.
const CLEANUP_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// The Chrome profile directory a session runs against.
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
///
/// # Two variants, and the removal belongs to one of them (#12)
///
/// A persistent profile is the entire point of #12: cookies, storefront
/// preferences and Cloudflare clearance are exactly what a throwaway directory
/// threw away every run. So the variant records who created the directory, and
/// [`Drop`] removes only the one this run made. A `owns: bool` beside a path
/// would be the same thing said worse — the variants make "removed on drop"
/// and "never removed" two types rather than two states of one.
enum ProfileDir {
    /// Created by this run under the temp directory, and removed when the
    /// session ends. `--no-profile`, and the fallback when a default
    /// persistent profile is already in use.
    Temporary { path: PathBuf },
    /// A directory that outlives the run: the one `--profile-dir` named, or
    /// the default under the data directory. **Never removed by this tool.**
    ///
    /// The lock is held for the life of the session and released by the OS when
    /// the process ends, however it ends. It is what stops two concurrent runs
    /// from sharing one profile, which Chrome answers with a `SingletonLock`
    /// failure that says nothing about who is holding it.
    Persistent { path: PathBuf, _lock: File },
}

/// The advisory lock file inside a persistent profile directory.
///
/// Ours rather than Chrome's `SingletonLock`, and the difference matters. That
/// file is a symlink naming a host and a pid, it survives a crash, and reading
/// it means guessing whether a pid from a previous boot is alive. An advisory
/// lock on a file we open is released by the kernel when the holder dies —
/// crash, `kill -9` and clean exit alike — so there is no stale state to
/// misread.
const PROFILE_LOCK: &str = ".iherb-cli-profile.lock";

impl ProfileDir {
    /// The profile directory this run should use, per [`ProfileChoice`].
    ///
    /// The three arms differ only in what happens when the directory is already
    /// in use, and that difference is #55's rule applied here: a path the
    /// caller stated binds, and one nobody stated may fall back.
    fn for_choice(choice: &ProfileChoice, data_dir: &Path) -> Result<Self, IherbError> {
        match choice {
            ProfileChoice::Throwaway => Self::temporary(),
            ProfileChoice::Stated(path) => match Self::persistent(path)? {
                Some(dir) => Ok(dir),
                // Stated, so it binds. Falling back here would hand the caller
                // a run against a profile that is not the one they named, and
                // the clearance they set up would appear not to work.
                None => Err(IherbError::BrowserLaunch(format!(
                    "The profile directory {} is in use by another iherb-cli \
                     run. A profile Chrome has open cannot be shared. Wait for \
                     that run, name a different --profile-dir, or pass \
                     --no-profile to use a throwaway profile.",
                    path.display()
                ))),
            },
            ProfileChoice::Default => {
                let path = ProfileChoice::default_dir(data_dir);
                match Self::persistent(&path)? {
                    Some(dir) => Ok(dir),
                    // Nobody named this one, so degrading is honest rather than
                    // a substitution — but it is said out loud, because the run
                    // that degrades is the run whose clearance will not persist.
                    None => {
                        tracing::warn!(
                            "The default profile directory {} is in use by another run; \
                             this run gets a throwaway profile, so nothing it does in \
                             the browser will be kept. Pass --profile-dir to use a \
                             second profile of your own.",
                            path.display()
                        );
                        Self::temporary()
                    }
                }
            }
        }
    }

    /// A profile directory no other run can be using.
    ///
    /// The name carries the pid and a millisecond timestamp to avoid the
    /// `SingletonLock` conflict two concurrent runs would otherwise hit, and to
    /// avoid inheriting a stale lock left behind by a previous one.
    fn temporary() -> Result<Self, IherbError> {
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
        Ok(Self::Temporary { path })
    }

    /// A persistent profile directory, locked for this run.
    ///
    /// `Ok(None)` means the directory exists and another run holds it — a fact
    /// the caller decides what to do about, because the answer depends on
    /// whether the caller named it. `Err` is reserved for a directory that
    /// could not be created or locked at all.
    fn persistent(path: &Path) -> Result<Option<Self>, IherbError> {
        std::fs::create_dir_all(path).map_err(|e| {
            IherbError::BrowserLaunch(format!(
                "Failed to create profile dir {}: {}",
                path.display(),
                e
            ))
        })?;

        let lock_path = path.join(PROFILE_LOCK);
        let lock = File::create(&lock_path).map_err(|e| {
            IherbError::BrowserLaunch(format!(
                "Failed to open the profile lock {}: {}",
                lock_path.display(),
                e
            ))
        })?;

        match lock.try_lock() {
            Ok(()) => Ok(Some(Self::Persistent {
                path: path.to_path_buf(),
                _lock: lock,
            })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(IherbError::BrowserLaunch(format!(
                "Failed to lock the profile dir {}: {}",
                path.display(),
                e
            ))),
        }
    }

    fn path(&self) -> &std::path::Path {
        match self {
            ProfileDir::Temporary { path } => path,
            ProfileDir::Persistent { path, .. } => path,
        }
    }

    /// Whether this run will remove the directory when it ends.
    ///
    /// Exposed so a test can assert the ownership rule against the session
    /// rather than against the flag that produced it (#12).
    fn is_temporary(&self) -> bool {
        matches!(self, ProfileDir::Temporary { .. })
    }
}

impl Drop for ProfileDir {
    fn drop(&mut self) {
        // The whole of #12's "a user-supplied profile dir is never deleted by
        // the tool": the removal is reachable from one variant, so there is no
        // path — panic, interrupt, or an ordinary close — on which a persistent
        // profile can be removed by accident.
        if let ProfileDir::Temporary { path } = self {
            remove_profile_dir(path);
        }
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
        Self::launch_into(
            chrome_path,
            config,
            ProfileDir::for_choice(&config.profile, &config.data_dir)?,
        )
        .await
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

    /// The profile directory Chrome was launched against.
    ///
    /// Exposed so a test can watch the directory rather than take the code's
    /// word for it (#46). Callers no longer need it to finish a cleanup someone
    /// else abandoned: dropping the session is the cleanup, on every path — and
    /// since #12 there are paths where the correct cleanup is none at all.
    pub fn profile_dir(&self) -> &std::path::Path {
        self.profile.path()
    }

    /// Whether this session will remove its profile directory when it ends.
    ///
    /// The ownership rule #12 asks for, asked of the session rather than of the
    /// flag that produced it: "a user-supplied profile dir is never deleted by
    /// the tool" is a property of what the session holds, and a test that
    /// checked the flag would be checking its own fixture.
    pub fn profile_is_temporary(&self) -> bool {
        self.profile.is_temporary()
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
        let closed = {
            let mut browser = self.browser.lock().await;
            // Bounded, because it used to not be (#12). `Browser::close` waits
            // for Chrome to answer a CDP request, and a Chrome that has stopped
            // answering never does — so a hung browser hung the CLI at exit
            // with nothing to interrupt but the process. The timeout turns that
            // into a kill, which is the answer `kill_on_drop` would have
            // reached eventually anyway; this only stops the wait from being
            // unbounded on the way there.
            let shutdown = async {
                browser.close().await.map_err(|e| {
                    IherbError::BrowserLaunch(format!("Failed to close browser: {}", e))
                })?;
                // Asking Chrome to close is not Chrome having closed, and the
                // difference is observable: the cookie jar, the preferences and
                // the storefront state a persistent profile exists to keep are
                // flushed on the way out (#12). A run that returned at the
                // acknowledgement dropped the `Browser` immediately afterwards,
                // and `kill_on_drop` then killed a process that was still
                // writing. Waiting is what makes "the profile survives the run"
                // true rather than usually true.
                let _ = browser.wait().await;
                Ok::<(), IherbError>(())
            };

            match tokio::time::timeout(CLOSE_TIMEOUT, shutdown).await {
                Ok(result) => result,
                Err(_) => {
                    tracing::warn!(
                        "The browser did not close within {:?}; killing it.",
                        CLOSE_TIMEOUT
                    );
                    match browser.kill().await {
                        Some(Err(e)) => Err(IherbError::BrowserLaunch(format!(
                            "The browser would not close and could not be killed: {}",
                            e
                        ))),
                        // `None` is chromiumoxide saying it no longer holds the
                        // child — which is the outcome asked for, not a failure.
                        _ => Ok(()),
                    }
                }
            }
        };

        // Explicit, because the ordering is the point: this kills Chrome and
        // then removes the profile directory, in that order, before the result
        // above is returned to a caller who might otherwise assume a failed
        // close means nothing was cleaned up.
        drop(self);

        closed
    }
}
