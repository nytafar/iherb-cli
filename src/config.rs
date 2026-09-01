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
    pub debug: bool,
    pub browser_path: Option<PathBuf>,
    pub cache_dir: PathBuf,
    pub data_dir: PathBuf,
}

#[derive(Debug, Deserialize, Default)]
struct ConfigFile {
    #[serde(default)]
    defaults: ConfigDefaults,
}

#[derive(Debug, Deserialize, Default)]
struct ConfigDefaults {
    country: Option<String>,
    currency: Option<String>,
    browser_path: Option<String>,
    delay_ms: Option<u64>,
    /// Same spelling as `--cache-ttl`: `30d`, `12h`. Parsed and reported like
    /// the flag, so a typo in the file fails the same way a typo on the command
    /// line does.
    cache_ttl: Option<String>,
}

impl AppConfig {
    /// Resolve the configuration from the global flags, the environment and the
    /// config file, in that order of priority.
    pub fn load(args: &GlobalArgs) -> Result<Self, IherbError> {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from(".cache"))
            .join("iherb-cli");
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
            .or_else(|| browser_path_env.map(PathBuf::from))
            .or_else(|| file_config.defaults.browser_path.map(PathBuf::from));

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

        let delay_ms = args.delay.or(file_config.defaults.delay_ms).unwrap_or(2000);

        Self::validate_country(&country)?;

        Ok(AppConfig {
            country,
            currency,
            cache_mode: CacheMode::from_flags(args.no_cache, args.refresh),
            cache_ttl,
            delay_ms,
            debug: args.debug,
            browser_path,
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
    toml::from_str(&content)
        .map_err(|e| IherbError::InvalidInput(format!("Could not parse {}: {}", path.display(), e)))
}
