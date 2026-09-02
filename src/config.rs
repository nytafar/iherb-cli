use crate::cli::GlobalArgs;
use crate::error::IherbError;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a cache entry stays usable when `--cache-ttl` says nothing.
///
/// Thirty days, which is what this tool has always used. It is far too long for
/// a price and about right for a supplement facts panel, and the two live in
/// one cached record — so telling them apart is a change to the model rather
/// than a second constant here (#15, DECISION-01). `--cache-ttl` is the honest
/// interim: one number, and the caller picks it.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How long a run waits between requests when `--delay` says nothing.
///
/// 500 ms since #11, down from 2000. The old number was never a politeness
/// figure: it was slept *before every navigation*, as a guess at how long a page
/// takes to render, and a 25-product comparison spent roughly a third of its
/// wall clock in it. Readiness selectors answer that question properly now, so
/// this is only the gap between one request and the next.
pub const DEFAULT_DELAY_MS: u64 = 500;

/// How many times a page is fetched before a run gives up, when `--attempts`
/// says nothing.
///
/// Three, which is what this tool has always done — it was the file-private
/// `NAVIGATION_RETRIES = 2` in `fetch.rs`, counted as retries on top of a first
/// try. Counted as a total here, because "attempts" is the word on the flag and
/// off-by-one between a flag and its constant is not worth inheriting.
pub const DEFAULT_NAVIGATION_ATTEMPTS: u32 = 3;

/// How many times a page is checked for a Cloudflare interstitial, when
/// `--cloudflare-attempts` says nothing.
///
/// Three, which is what the hardcoded `MAX_CLOUDFLARE_RETRIES` was. #23's last
/// acceptance criterion is that this number stop being hardcoded: how much of a
/// rate-limit budget one page is worth is the caller's question, not this
/// file's.
pub const DEFAULT_CLOUDFLARE_ATTEMPTS: u32 = 3;

/// What a run is allowed to do with the cache.
///
/// Three states rather than a `no_cache: bool`, because the bool could not say
/// what the flag promised. `Cache::new(dir, no_cache)` set `read_enabled:
/// !no_cache` and wrote regardless, so `--no-cache` — documented as "bypass
/// local cache" — still put files on disk. That is a defensible *behaviour*;
/// it was the wrong name for it (#22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// Read and write. What a run with no cache flags does.
    ReadWrite,
    /// Skip reads, write the result. `--refresh`, and what `--no-cache` used
    /// to do.
    Refresh,
    /// Neither read nor write. `--no-cache`, meaning what it says.
    Off,
}

impl CacheMode {
    /// What the two flags resolve to.
    ///
    /// `--no-cache` wins over `--refresh` when both are given: it is the
    /// stronger of the two requests, and refusing the combination would fail a
    /// command line that is merely redundant.
    pub fn from_flags(no_cache: bool, refresh: bool) -> Self {
        match (no_cache, refresh) {
            (true, _) => CacheMode::Off,
            (false, true) => CacheMode::Refresh,
            (false, false) => CacheMode::ReadWrite,
        }
    }

    pub fn reads(self) -> bool {
        matches!(self, CacheMode::ReadWrite)
    }

    pub fn writes(self) -> bool {
        matches!(self, CacheMode::ReadWrite | CacheMode::Refresh)
    }
}

/// Parse `30d`, `12h`, `45m`, `90s` into a [`Duration`].
///
/// A unit is required. A bare `30` could mean seconds or days depending on who
/// is reading, and a cache TTL that is wrong by a factor of 86,400 in either
/// direction is worth two more keystrokes.
pub fn parse_duration(text: &str) -> Result<Duration, IherbError> {
    let text = text.trim();
    let (digits, unit) = text.split_at(
        text.find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len()),
    );
    let bad = || {
        IherbError::InvalidInput(format!(
            "'{}' is not a duration. Write a number and a unit: 90s, 45m, 12h, 30d, 2w.",
            text
        ))
    };
    let value: u64 = digits.parse().map_err(|_| bad())?;
    let seconds = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        "w" => 7 * 24 * 60 * 60,
        _ => return Err(bad()),
    };
    value
        .checked_mul(seconds)
        .map(Duration::from_secs)
        .ok_or_else(|| {
            IherbError::InvalidInput(format!(
                "'{}' is longer than this tool can represent.",
                text
            ))
        })
}

/// Where a caller named the browser executable.
///
/// Carried rather than discarded (#55) for two reasons, and the second is the
/// one that matters. The first is that an error can then say *which* of the
/// three to go and edit. The second is that the source is the only thing that
/// could have justified treating the three differently — and having it in hand
/// is what makes the decision below a decision rather than an accident of them
/// sharing a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserPathSource {
    /// `--browser-path`, on this invocation.
    Flag,
    /// The `IHERB_BROWSER_PATH` environment variable.
    Env,
    /// `browser_path` under `[defaults]` in the named config file.
    ConfigFile(PathBuf),
}

impl BrowserPathSource {
    /// How to name this source to someone who has to go and correct it.
    pub fn describe(&self) -> String {
        match self {
            BrowserPathSource::Flag => "--browser-path".to_string(),
            BrowserPathSource::Env => "IHERB_BROWSER_PATH".to_string(),
            BrowserPathSource::ConfigFile(path) => {
                format!("browser_path in {}", path.display())
            }
        }
    }
}

/// A browser executable the caller named, and where they named it.
///
/// # All three sources bind, and that is the decision (#55)
///
/// `--browser-path /nonexistent` used to exit 0 with `ok: true` and a full
/// record, having quietly used system Chrome. #55 asks that an explicit flag
/// stop doing that, and asks separately that whatever happens to the
/// environment variable and the config file be *decided* rather than inherited
/// from the three sharing an `Option<PathBuf>`.
///
/// They are decided the same way: **a browser you named is the browser that
/// runs, or the run fails.** The argument for exempting the config file is that
/// a months-old entry pointing at a moved binary would break a setup that a
/// fall-through would have rescued. It does not survive contact with what the
/// fall-through actually costs. The substitution is silent, the record that
/// comes back is indistinguishable from one the named browser produced, and
/// #12 makes that concretely dangerous: Cloudflare clearance earned in a
/// profile belongs to *a browser*, so a run that silently used a different one
/// looks like clearance that stopped working. Against that, a hard failure
/// that names the path and the file to edit costs one correction, once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatedBrowserPath {
    pub path: PathBuf,
    pub source: BrowserPathSource,
}

impl StatedBrowserPath {
    pub fn new(path: PathBuf, source: BrowserPathSource) -> Self {
        Self { path, source }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Which Chrome profile directory a run uses (#12).
///
/// Every run used to get a fresh throwaway profile under the temp directory,
/// deleted on the way out — a cold, cookie-less, history-less browser, which is
/// the fingerprint Cloudflare scores worst, and clearance earned by one run
/// thrown away before the next could use it. Storefront preferences could not
/// persist either.
///
/// Three states rather than an `Option<PathBuf>`, because "no directory named"
/// and "no persistence wanted" are different requests and the second has to be
/// sayable. The default is now persistent, so the benefit is on without a flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileChoice {
    /// `--profile-dir <path>`. The caller named it, so it binds the way a
    /// stated browser path does (#55): the tool uses that directory or fails,
    /// and it never removes it.
    Stated(PathBuf),
    /// No flag. A persistent profile under the data directory, which degrades
    /// to a throwaway one with a warning if another run holds it.
    Default,
    /// `--no-profile`. A throwaway profile under the temp directory, removed on
    /// the way out. What every run did before #12.
    Throwaway,
}

impl ProfileChoice {
    /// What the two flags resolve to.
    ///
    /// The pair is a contradiction rather than a redundancy, so it is refused
    /// rather than ordered by strength the way `--no-cache` and `--refresh`
    /// are: there is no reading of "use this directory, and use no directory"
    /// that honours both.
    pub fn from_flags(profile_dir: Option<&Path>, no_profile: bool) -> Result<Self, IherbError> {
        match (profile_dir, no_profile) {
            (Some(path), false) => Ok(ProfileChoice::Stated(path.to_path_buf())),
            (None, true) => Ok(ProfileChoice::Throwaway),
            (None, false) => Ok(ProfileChoice::Default),
            (Some(path), true) => Err(IherbError::InvalidInput(format!(
                "--profile-dir {} and --no-profile ask for opposite things: a \
                 profile kept at that path, and no profile kept at all. Pass \
                 one.",
                path.display()
            ))),
        }
    }

    /// Where the default persistent profile lives, under the data directory
    /// `data_dir` names.
    pub fn default_dir(data_dir: &Path) -> PathBuf {
        data_dir.join("profile")
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub country: String,
    /// The currency `--currency` requires the storefront to price in, or `None`
    /// when the caller did not say.
    ///
    /// An `Option` with no default, because the default cannot be a currency
    /// (#5). This used to be a `String` defaulting to `"USD"`, which was safe
    /// only because the value was a label of last resort that the page usually
    /// overrode. As a requirement, a `"USD"` default would make every
    /// non-US storefront fail out of the box.
    pub currency: Option<String>,
    pub cache_mode: CacheMode,
    pub cache_ttl: Duration,
    pub delay_ms: u64,
    /// How many times one page is fetched before the run gives up (#23).
    ///
    /// A total, so one is a legal value and means "try once". Never zero:
    /// [`AppConfig::load`] refuses it, because a run that is not allowed to
    /// look at the page cannot report anything about it.
    pub attempts: u32,
    /// How many times one page is checked for a Cloudflare interstitial (#23).
    pub cloudflare_attempts: u32,
    /// Verbose logging and the HTML dump. Says nothing about the window (#62).
    pub debug: bool,
    /// A browser window you can see. Says nothing about logging (#62).
    pub headful: bool,
    /// Per-phase navigation durations on stderr (#11).
    ///
    /// Its own flag rather than part of `--debug`, for the reason #62 split the
    /// window off it: a caller that wants to know where the seconds went does
    /// not necessarily want every debug line and an HTML dump per page, and one
    /// line per navigation is cheap enough to ask for on its own.
    pub timing: bool,
    /// The browser executable the caller named, with the source that named it.
    ///
    /// `None` means nobody named one, which is the only case
    /// [`crate::browser::resolve::resolve_chrome`] is still free to fall
    /// through on (#55).
    pub browser_path: Option<StatedBrowserPath>,
    /// Which Chrome profile directory this run uses (#12).
    pub profile: ProfileChoice,
    pub cache_dir: PathBuf,
    pub data_dir: PathBuf,
}

#[derive(Debug, Deserialize, Default)]
struct ConfigFile {
    #[serde(default)]
    defaults: ConfigDefaults,
    /// The file these defaults were read from, so an error about one of them
    /// can name the file to edit (#55). Not a key in the file: filled in by
    /// [`read_config_file`], and empty for the defaults nobody read.
    #[serde(skip)]
    path: PathBuf,
}

#[derive(Debug, Deserialize, Default)]
struct ConfigDefaults {
    country: Option<String>,
    currency: Option<String>,
    browser_path: Option<String>,
    delay_ms: Option<u64>,
    /// Same meaning as `--attempts` and `--cloudflare-attempts`, and refused
    /// the same way when zero.
    attempts: Option<u32>,
    cloudflare_attempts: Option<u32>,
    /// Same spelling as `--cache-ttl`: `30d`, `12h`. Parsed and reported like
    /// the flag, so a typo in the file fails the same way a typo on the command
    /// line does.
    cache_ttl: Option<String>,
}

/// The cache directory this tool uses on this platform.
///
/// `~/Library/Caches/iherb-cli` on macOS, `~/.cache/iherb-cli` on Linux, and a
/// relative `.cache/iherb-cli` when the platform will not say. Its own function
/// rather than a line inside [`AppConfig::load`] because the HTML dumps need
/// the same answer and are written from the scrapers, which have no config in
/// hand (#63). One resolution, two callers, rather than two resolutions that
/// can drift.
pub fn resolve_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from(".cache"))
        .join("iherb-cli")
}

/// Where `--debug` writes the HTML it fetched.
///
/// A `dumps` subdirectory of the cache directory, so the dumps inherit the
/// platform logic that already existed instead of landing in a hardcoded `/tmp`
/// path (#63), and so `cache path` names the directory they are under.
///
/// **They are not cache entries and nothing here treats them as any.**
/// `cache stats` does not count them and `cache clear` does not remove them:
/// both are documented as touching regular `.json` files sitting *directly* in
/// the cache directory, never a subdirectory, and that guarantee is worth more
/// than sweeping the dumps with it. Removing them is `rm -r` on this path.
pub fn dumps_dir() -> PathBuf {
    resolve_cache_dir().join("dumps")
}

impl AppConfig {
    /// Resolve the configuration from the global flags, the environment and the
    /// config file, in that order of priority.
    pub fn load(args: &GlobalArgs) -> Result<Self, IherbError> {
        let cache_dir = resolve_cache_dir();
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from(".local/share"))
            .join("iherb-cli");

        let file_config = match args.config.as_deref() {
            // A path the caller named by hand. A missing or malformed file is
            // an error rather than a silent fall-through to the defaults:
            // asking for a file and being given the defaults instead is how a
            // test passes for the wrong reason (#22).
            Some(path) => read_config_file(path)?,
            None => load_default_config_file(
                &dirs::config_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("iherb-cli"),
            ),
        };

        // Priority: CLI flags → env vars → config file → defaults
        let browser_path_env = std::env::var("IHERB_BROWSER_PATH").ok();
        let country_env = std::env::var("IHERB_COUNTRY").ok();
        let currency_env = std::env::var("IHERB_CURRENCY").ok();

        // `--browser-path` sits at the head of the chain the other settings
        // already had one of. It did not exist at all before #22: the executable
        // could be named by `IHERB_BROWSER_PATH` or by the config file, so an
        // agent that cannot set an environment variable on a subprocess had no
        // way to point at a browser and fell through to downloading Chrome.
        let browser_path = args
            .browser_path
            .clone()
            .map(|path| StatedBrowserPath::new(path, BrowserPathSource::Flag))
            .or_else(|| {
                browser_path_env
                    .map(|path| StatedBrowserPath::new(PathBuf::from(path), BrowserPathSource::Env))
            })
            .or_else(|| {
                file_config.defaults.browser_path.clone().map(|path| {
                    StatedBrowserPath::new(
                        PathBuf::from(path),
                        BrowserPathSource::ConfigFile(file_config.path.clone()),
                    )
                })
            });

        let cache_ttl = match args.cache_ttl.as_deref() {
            Some(text) => parse_duration(text)?,
            None => match file_config.defaults.cache_ttl.as_deref() {
                Some(text) => parse_duration(text)?,
                None => DEFAULT_CACHE_TTL,
            },
        };

        let country = args
            .country
            .clone()
            .or(country_env)
            .or(file_config.defaults.country)
            .unwrap_or_else(|| "us".to_string());

        // No `unwrap_or`: saying nothing is a real answer here, and the only
        // safe one. See [`AppConfig::currency`].
        let currency = args
            .currency
            .clone()
            .or(currency_env)
            .or(file_config.defaults.currency)
            .map(|c| c.trim().to_uppercase())
            .filter(|c| !c.is_empty());

        // 500, not the 2000 it was. The old default was doing two jobs — a
        // guess at page-load time *and* politeness between requests — and #11
        // moved the first job to the readiness selectors. What is left is the
        // gap between one request and the next, and half a second of that is
        // still polite for the handful of pages a run fetches.
        let delay_ms = args
            .delay
            .or(file_config.defaults.delay_ms)
            .unwrap_or(DEFAULT_DELAY_MS);

        // Flag, then config file, then the default — the same chain `--delay`
        // has, and for the same reason: an agent that cannot set an environment
        // variable on a subprocess still has a flag, and a person who always
        // wants the same number has a file (#22, #23).
        let attempts = args
            .attempts
            .or(file_config.defaults.attempts)
            .unwrap_or(DEFAULT_NAVIGATION_ATTEMPTS);
        let cloudflare_attempts = args
            .cloudflare_attempts
            .or(file_config.defaults.cloudflare_attempts)
            .unwrap_or(DEFAULT_CLOUDFLARE_ATTEMPTS);

        Self::validate_country(&country)?;
        validate_attempts("--attempts", attempts)?;
        validate_attempts("--cloudflare-attempts", cloudflare_attempts)?;

        Ok(AppConfig {
            country,
            currency,
            cache_mode: CacheMode::from_flags(args.no_cache, args.refresh),
            cache_ttl,
            delay_ms,
            attempts,
            cloudflare_attempts,
            debug: args.debug,
            headful: args.headful,
            timing: args.timing,
            browser_path,
            profile: ProfileChoice::from_flags(args.profile_dir.as_deref(), args.no_profile)?,
            cache_dir,
            data_dir,
        })
    }

    pub fn validate_country(country: &str) -> Result<(), IherbError> {
        const KNOWN_COUNTRIES: &[&str] = &[
            "us", "ca", "au", "nz", "sg", "hk", "tw", "kr", "jp", "sa", "ae", "kw", "il", "de",
            "fr", "es", "it", "nl", "be", "at", "ch", "se", "no", "dk", "fi", "pl", "cz", "ie",
            "pt", "gr", "ru", "tr", "in", "th", "my", "ph", "id", "vn", "br", "mx", "cl", "co",
            "ar", "za", "eg", "ng", "ke", "cn",
        ];
        if !KNOWN_COUNTRIES.contains(&country) {
            return Err(IherbError::InvalidInput(format!(
                "Unknown country code '{}'. iHerb may not support this subdomain. Known codes include: us, ca, de, fr, ch, au, jp, kr, etc.",
                country
            )));
        }
        Ok(())
    }

    pub fn base_url(&self) -> String {
        if self.country == "us" {
            "https://www.iherb.com".to_string()
        } else {
            format!("https://{}.iherb.com", self.country)
        }
    }
}

/// Zero attempts is refused rather than clamped to one (#23).
///
/// Clamping would be the friendlier reading, and it is the wrong one: a caller
/// who wrote `--attempts 0` meant something, and what they meant is not "try
/// once". Silently doing the opposite of a number they typed is how a run that
/// was supposed to be a dry check turns into a request against iHerb.
fn validate_attempts(flag: &str, attempts: u32) -> Result<(), IherbError> {
    if attempts == 0 {
        return Err(IherbError::InvalidInput(format!(
            "{} must be at least 1. A page that is never looked at cannot be \
             reported on, and 0 is not a way to skip the request.",
            flag
        )));
    }
    Ok(())
}

/// The config file at the default location, which most runs do not have.
///
/// Absent is normal and silent. Present-but-unreadable is not: it used to fall
/// through to the defaults without a word, so a typo in the file looked exactly
/// like having no file. It still does not fail the run — the default path is
/// something the user may not know exists — but it says so on stderr.
fn load_default_config_file(config_dir: &Path) -> ConfigFile {
    let config_path = config_dir.join("config.toml");
    if !config_path.exists() {
        return ConfigFile::default();
    }
    match read_config_file(&config_path) {
        Ok(config) => config,
        Err(e) => {
            tracing::warn!("Ignoring {}: {}", config_path.display(), e);
            ConfigFile::default()
        }
    }
}

/// One config file, read and parsed, with both failures reported.
fn read_config_file(path: &Path) -> Result<ConfigFile, IherbError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        IherbError::InvalidInput(format!("Could not read {}: {}", path.display(), e))
    })?;
    let mut config: ConfigFile = toml::from_str(&content).map_err(|e| {
        IherbError::InvalidInput(format!("Could not parse {}: {}", path.display(), e))
    })?;
    config.path = path.to_path_buf();
    Ok(config)
}
