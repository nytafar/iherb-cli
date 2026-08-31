use crate::cli::SortOrder;
use crate::error::IherbError;
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The cache layout generation, carried in every file name.
///
/// Bumping it abandons every entry on disk rather than reusing it, which is
/// what a key derivation change has to do: an entry written under an older,
/// coarser key cannot be shown to belong to the request now asking for it.
/// `v2` was #1 — the generation in which an entry names the storefront it came
/// from. `v3` was #5's first half, and it abandoned the `v2` entries for a
/// different reason: not because the key was too coarse, but because **the
/// records themselves held a currency nobody read**. Every path used to
/// substitute one when the page published none — a hardcoded `"USD"`, or
/// whatever label was passed to `--currency` — so a `v2` file can say
/// `"currency": "CHF"` about a US price with nothing in it to say otherwise.
///
/// `v4` is #5's second half, and it is a key change after all. When `v3`
/// landed, `--currency` was an assertion about the storefront: it could reject
/// an answer but not change which document was fetched, so keying on it would
/// have filed one fetch under two names. That reasoning was right then and is
/// wrong now. `--currency` sets iHerb's own storefront-preference cookies
/// before the request, so `--currency NOK` and `--currency EUR` fetch *different
/// documents* — measured: product 12949 is NOK 880.63, €76.57 and $64.56 on the
/// three storefronts. A key that leaves the currency out claims those are the
/// same document, which is #1's bug in a second dimension.
///
/// Older files — `product_61864.json`, `v2_…` and `v3_…` alike — are never read
/// again and are left where they are; nothing deletes them, so a stale entry
/// costs disk until the user clears the cache directory (#22 adds the command
/// for that).
const CACHE_GENERATION: &str = "v4";

/// What a cache file name says in place of a currency when `--currency` was not
/// given.
///
/// Lower case, and every currency this reaches has been upper-cased by
/// [`crate::config::AppConfig::load`] — so `--currency any` cannot be mistaken
/// for the absence of a currency the way `--category ""` could be mistaken for
/// no category before #4.
const NO_CURRENCY_REQUESTED: &str = "any";

/// Where a fetched artefact lives in the cache.
///
/// One value per kind of cacheable thing, so a new command declares its cache
/// identity instead of adding another pair of `get_x`/`set_x` methods.
///
/// Every variant names a `country` and a `currency`, because both are part of
/// the request. The country picks the subdomain; the currency is set on iHerb's
/// own preference cookies before the page is fetched, and the storefront prices
/// in it. A key that leaves either out claims two different documents are the
/// same document — #1 for the country, and the same bug one dimension over for
/// the currency (#5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheKey {
    Product {
        country: String,
        /// The currency `--currency` asked the storefront for, or `None` for
        /// whatever it prices in by default. Not the currency the record came
        /// back holding: this names the *request*, and the answer to it lives
        /// in the entry.
        currency: Option<String>,
        product_id: String,
    },
    Search {
        country: String,
        currency: Option<String>,
        query: String,
        sort: SortOrder,
        category: Option<String>,
    },
}

impl CacheKey {
    /// The cache file name.
    ///
    /// Every input that changes *which document is fetched* has to appear here,
    /// or two different documents share one entry and whichever was written
    /// first is served for both. #1 was exactly that: the key held no country,
    /// so `--country ch` was handed the cached US record — a USD price labelled
    /// USD, from a `www.iherb.com` URL, with a plausible `Data from` timestamp
    /// and no error, for the 30 days of the TTL.
    ///
    /// Changing the derivation orphans every entry users already have, which is
    /// why the name carries [`CACHE_GENERATION`] rather than being edited in
    /// place, and why #8 pins the derivation from `tests/`. Changing what a
    /// stored *record* means orphans them too, for the same reason and through
    /// the same mechanism: see [`CACHE_GENERATION`] for why #5 bumped it
    /// without touching a single field below.
    pub fn file_name(&self) -> String {
        match self {
            CacheKey::Product {
                country,
                currency,
                product_id,
            } => format!(
                "{}_product_{}_{}_{}.json",
                CACHE_GENERATION,
                country,
                currency.as_deref().unwrap_or(NO_CURRENCY_REQUESTED),
                product_id
            ),
            CacheKey::Search {
                country,
                currency,
                query,
                sort,
                category,
            } => {
                // Every field is delimited by a NUL that cannot occur in any of
                // them, and the optional one is tagged present or absent, so
                // two distinct requests cannot hash alike — not by running
                // together at a boundary, and not by `--category ""` looking
                // like no category at all. Same failure class as the country,
                // one storefront smaller.
                let mut hasher = Sha256::new();
                hasher.update(country.as_bytes());
                hasher.update(b"\0");
                hasher.update(
                    currency
                        .as_deref()
                        .unwrap_or(NO_CURRENCY_REQUESTED)
                        .as_bytes(),
                );
                hasher.update(b"\0");
                hasher.update(query.as_bytes());
                hasher.update(b"\0");
                hasher.update(sort.as_cache_key().as_bytes());
                hasher.update(b"\0");
                match category {
                    Some(cat) => {
                        hasher.update(b"1");
                        hasher.update(cat.as_bytes());
                    }
                    None => hasher.update(b"0"),
                }
                let result = hasher.finalize();
                // 16 hex chars.
                format!(
                    "{}_search_{}.json",
                    CACHE_GENERATION,
                    hex::encode(&result[..8])
                )
            }
        }
    }

    /// How this kind of entry is described in log messages.
    pub fn label(&self) -> &'static str {
        match self {
            CacheKey::Product { .. } => "product data",
            CacheKey::Search { .. } => "search results",
        }
    }
}

pub struct Cache {
    dir: PathBuf,
    read_enabled: bool,
}

/// Result from a cache read, including the data and when it was cached.
pub struct CacheHit<T> {
    pub data: T,
    pub cached_at: SystemTime,
}

const CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60); // 30 days

impl Cache {
    /// Create a cache. When `no_cache` is true, reads are skipped but writes still happen.
    pub fn new(cache_dir: PathBuf, no_cache: bool) -> Self {
        Self {
            dir: cache_dir,
            read_enabled: !no_cache,
        }
    }

    /// Read an entry, or `None` if it is missing, stale, unreadable, or reads
    /// are disabled by `--no-cache`.
    pub fn get<T: DeserializeOwned>(&self, key: &CacheKey) -> Option<CacheHit<T>> {
        if !self.read_enabled {
            return None;
        }
        let path = self.dir.join(key.file_name());
        self.read_cached(&path, CACHE_TTL)
    }

    /// Write an entry. Writes happen even under `--no-cache`, which only
    /// suppresses reads.
    pub fn set<T: Serialize>(&self, key: &CacheKey, data: &T) -> Result<(), IherbError> {
        let path = self.dir.join(key.file_name());
        self.write_cached(&path, data)
    }

    fn read_cached<T: DeserializeOwned>(&self, path: &Path, ttl: Duration) -> Option<CacheHit<T>> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified = metadata.modified().ok()?;
        let age = SystemTime::now().duration_since(modified).ok()?;
        if age > ttl {
            tracing::debug!("Cache expired for {}", path.display());
            return None;
        }
        let content = std::fs::read_to_string(path).ok()?;
        match serde_json::from_str(&content) {
            Ok(data) => {
                tracing::info!("Cache hit for {}", path.display());
                Some(CacheHit {
                    data,
                    cached_at: modified,
                })
            }
            Err(e) => {
                tracing::warn!("Cache parse error for {}: {}", path.display(), e);
                None
            }
        }
    }

    fn write_cached<T: Serialize>(&self, path: &Path, data: &T) -> Result<(), IherbError> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| IherbError::Cache(format!("Failed to create cache dir: {}", e)))?;
        let content = serde_json::to_string_pretty(data)?;
        std::fs::write(path, content)
            .map_err(|e| IherbError::Cache(format!("Failed to write cache: {}", e)))?;
        tracing::debug!("Cached to {}", path.display());
        Ok(())
    }
}
