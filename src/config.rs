use crate::error::IherbError;
use serde::Deserialize;
use std::path::{Path, PathBuf};

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
    pub no_cache: bool,
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
}

impl AppConfig {
    pub fn load(
        country: Option<String>,
        currency: Option<String>,
        no_cache: bool,
        delay: Option<u64>,
        debug: bool,
    ) -> Result<Self, IherbError> {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("iherb-cli");
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from(".cache"))
            .join("iherb-cli");
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from(".local/share"))
            .join("iherb-cli");

        let file_config = load_config_file(&config_dir);

        // Priority: CLI flags → env vars → config file → defaults
        let browser_path_env = std::env::var("IHERB_BROWSER_PATH").ok();
        let country_env = std::env::var("IHERB_COUNTRY").ok();
        let currency_env = std::env::var("IHERB_CURRENCY").ok();

        let browser_path = browser_path_env
            .or(file_config.defaults.browser_path)
            .map(PathBuf::from);

        let country = country
            .or(country_env)
            .or(file_config.defaults.country)
            .unwrap_or_else(|| "us".to_string());

        // No `unwrap_or`: saying nothing is a real answer here, and the only
        // safe one. See [`AppConfig::currency`].
        let currency = currency
            .or(currency_env)
            .or(file_config.defaults.currency)
            .map(|c| c.trim().to_uppercase())
            .filter(|c| !c.is_empty());

        let delay_ms = delay.or(file_config.defaults.delay_ms).unwrap_or(2000);

        Self::validate_country(&country)?;

        Ok(AppConfig {
            country,
            currency,
            no_cache,
            delay_ms,
            debug,
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
            return Err(IherbError::Navigation(format!(
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

fn load_config_file(config_dir: &Path) -> ConfigFile {
    let config_path = config_dir.join("config.toml");
    if config_path.exists() {
        match std::fs::read_to_string(&config_path) {
            Ok(content) => toml::from_str(&content).unwrap_or_default(),
            Err(_) => ConfigFile::default(),
        }
    } else {
        ConfigFile::default()
    }
}
