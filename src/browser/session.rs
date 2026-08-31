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

pub struct BrowserSession {
    browser: Arc<Mutex<Browser>>,
    _handle: tokio::task::JoinHandle<()>,
    user_data_dir: PathBuf,
}

impl BrowserSession {
    pub async fn launch(chrome_path: PathBuf, config: &AppConfig) -> Result<Self, IherbError> {
        // Create a unique user data directory to avoid SingletonLock conflicts
        // when multiple instances run concurrently or after a stale lock is left behind.
        let user_data_dir = std::env::temp_dir().join(format!(
            "iherb-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        ));
        std::fs::create_dir_all(&user_data_dir).map_err(|e| {
            IherbError::BrowserLaunch(format!(
                "Failed to create user data dir {}: {}",
                user_data_dir.display(),
                e
            ))
        })?;

        // Nothing else can clean this directory up if the launch fails: `close`
        // and `Drop` both belong to a session that, in that case, never exists
        // (#46). Chrome that will not start is exactly when a launch is retried,
        // so this is the leak that repeats.
        match Self::launch_into(chrome_path, config, user_data_dir.clone()).await {
            Ok(session) => Ok(session),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&user_data_dir);
                Err(e)
            }
        }
    }

    /// The launch proper, on a profile directory that already exists.
    async fn launch_into(
        chrome_path: PathBuf,
        config: &AppConfig,
        user_data_dir: PathBuf,
    ) -> Result<Self, IherbError> {
        let mut builder = BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(user_data_dir.clone())
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
        // browser: on the `--debug` path nothing was set, the default mode
        // still appended `--headless`, and the flag only ever changed logging.
        // It looked right on the headless path purely by accident — the mode
        // appends a *valueless* `headless` key, which merges into the `new`
        // value already under that key and comes out as `--headless=new`.
        //
        // `--debug` has to be genuinely headful: #12's `setup` command exists so
        // a human can complete a Cloudflare challenge in a window they can see.
        builder = if config.debug {
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
            user_data_dir,
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
    /// Exposed so that a caller who abandons [`Self::close`] can still finish
    /// what it started: dropping the session kills Chrome, but its subprocesses
    /// keep writing here for a moment afterwards, and the removal `Drop`
    /// attempts in that moment is the one that leaves a partial directory (#46).
    pub fn profile_dir(&self) -> &std::path::Path {
        &self.user_data_dir
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

    pub async fn close(self) -> Result<(), IherbError> {
        let mut browser = self.browser.lock().await;
        browser
            .close()
            .await
            .map_err(|e| IherbError::BrowserLaunch(format!("Failed to close browser: {}", e)))?;

        // Drop the browser handle so Chrome subprocesses can fully exit
        drop(browser);

        // Give Chrome subprocesses time to release file locks
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Clean up the temporary user data directory with retry
        if self.user_data_dir.exists() {
            for attempt in 1..=3 {
                match std::fs::remove_dir_all(&self.user_data_dir) {
                    Ok(_) => break,
                    Err(e) if attempt < 3 => {
                        tracing::debug!(
                            "Cleanup attempt {}/3 for {}: {}, retrying...",
                            attempt,
                            self.user_data_dir.display(),
                            e
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                    Err(_) => {
                        // Final attempt failed — silently ignore. The OS will clean /tmp.
                        tracing::debug!(
                            "Could not clean up temp dir {}, will be cleaned by OS",
                            self.user_data_dir.display()
                        );
                    }
                }
            }
        }

        Ok(())
    }
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        if self.user_data_dir.exists() {
            let _ = std::fs::remove_dir_all(&self.user_data_dir);
        }
    }
}
