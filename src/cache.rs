use crate::cli::SortOrder;
use crate::config::CacheMode;
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

/// A cache entry that did not get written.
///
/// Deliberately **not** an [`crate::error::IherbError`], and that is the point
/// rather than an oversight. The taxonomy documented a `cache_error` (32) and a
/// `json_error` (40) that no run could ever exit on, because the cache is an
/// optimization: a read that fails is a miss, and a write that fails is a log
/// line beside a perfectly good result. Neither should fail a run — a full disk
/// is not a reason to throw away a page we already fetched — so this is its own
/// type rather than a code in a table that claimed it could and never did.
///
/// # What the type does and does not buy
///
/// It buys the compiler's help at the `?`: a `Result<_, CacheWriteFailed>` is
/// not a `Result<_, IherbError>`, so nothing can propagate one *as* a member of
/// the taxonomy, and [`crate::error::IherbError::kind`] never has to answer for
/// it.
///
/// It does **not** make the pipeline unreachable, and an earlier draft of this
/// comment claimed it did. Rust converts any `std::error::Error` into an
/// `anyhow::Error`, so `cache.set(..)?` inside an `anyhow`-returning function
/// compiles, and the error it produces carries no [`crate::error::IherbError`]
/// anywhere in its chain — which is precisely what
/// [`crate::error::classify_error`] reports as `internal_error` (70). A full
/// disk would then tell a caller to file a bug about this tool.
///
/// What actually keeps that from happening is a control-flow boundary, not a
/// type-level impossibility: the one call site, `src/fetch.rs:364`, handles the
/// write failure where it happens — `if let Err(e) = cache.set(..)`, logged and
/// dropped — instead of propagating it. Keep the `?` off that call and the
/// promise holds.
///
/// The same reasoning covers a read: [`Cache::get`] returns `Option`, and a
/// file that will not parse is `None` — a miss, refetched, with a warning.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CacheWriteFailed(String);

pub struct Cache {
    dir: PathBuf,
    mode: CacheMode,
    ttl: Duration,
}

/// Result from a cache read, including the data and when it was cached.
pub struct CacheHit<T> {
    pub data: T,
    pub cached_at: SystemTime,
}

impl Cache {
    /// Create a cache with an explicit policy.
    ///
    /// The mode used to be a `no_cache: bool` that disabled reads and left
    /// writes alone, so the flag named after "no cache" wrote files (#22). See
    /// [`CacheMode`].
    pub fn new(cache_dir: PathBuf, mode: CacheMode, ttl: Duration) -> Self {
        Self {
            dir: cache_dir,
            mode,
            ttl,
        }
    }

    /// The directory this cache lives in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Read an entry, or `None` if it is missing, stale, unreadable, or reads
    /// are disabled by `--refresh` or `--no-cache`.
    pub fn get<T: DeserializeOwned>(&self, key: &CacheKey) -> Option<CacheHit<T>> {
        if !self.mode.reads() {
            return None;
        }
        let path = self.dir.join(key.file_name());
        self.read_cached(&path, self.ttl)
    }

    /// Write an entry, unless `--no-cache` asked for the cache to be left
    /// alone entirely.
    ///
    /// `--refresh` still writes: skipping the read and keeping the answer is
    /// the whole point of it.
    pub fn set<T: Serialize>(&self, key: &CacheKey, data: &T) -> Result<(), CacheWriteFailed> {
        if !self.mode.writes() {
            return Ok(());
        }
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

    fn write_cached<T: Serialize>(&self, path: &Path, data: &T) -> Result<(), CacheWriteFailed> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| CacheWriteFailed(format!("Failed to create cache dir: {}", e)))?;
        let content = serde_json::to_string_pretty(data)
            .map_err(|e| CacheWriteFailed(format!("Failed to serialize the entry: {}", e)))?;
        std::fs::write(path, content)
            .map_err(|e| CacheWriteFailed(format!("Failed to write cache: {}", e)))?;
        tracing::debug!("Cached to {}", path.display());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Cache management: `cache path`, `cache stats`, `cache clear` (#22)
// ---------------------------------------------------------------------------

/// One file in the cache directory, as the management commands see it.
///
/// Enumerated rather than deserialized. `stats` and `clear` are file operations
/// over a directory of JSON blobs; opening every entry to read a country out of
/// it would make `cache stats` cost as much as a fetch, and #27 — a database
/// that could answer such questions cheaply — is parked.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub name: String,
    pub bytes: u64,
    pub modified: SystemTime,
    /// The storefront this entry belongs to, when the file name says.
    ///
    /// `None` for a search entry, and that is a property of the key rather than
    /// a gap here: a product entry is named
    /// `v4_product_<country>_<currency>_<id>.json`, but a search entry is
    /// `v4_search_<hash>.json` — the country went into the hash and cannot be
    /// read back out. `clear --country` reports these as unattributable instead
    /// of guessing or quietly skipping them.
    pub country: Option<String>,
}

impl CacheEntry {
    /// The country a cache file name states, if it states one.
    fn country_from_name(name: &str) -> Option<String> {
        // `v4_product_no_NOK_12949.json`
        let rest = name.split_once("_product_")?.1;
        let country = rest.split('_').next()?;
        (!country.is_empty()).then(|| country.to_string())
    }
}

/// What the cache directory holds.
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub dir: PathBuf,
    pub entries: usize,
    pub bytes: u64,
    pub oldest: Option<SystemTime>,
    pub newest: Option<SystemTime>,
}

/// Which entries `cache clear` should remove.
#[derive(Debug, Clone, Default)]
pub struct ClearFilter {
    /// Only entries last written before this instant.
    pub older_than: Option<SystemTime>,
    /// Only entries whose file name names this country.
    pub country: Option<String>,
}

impl ClearFilter {
    /// Whether any filter was given at all. An unfiltered clear removes
    /// everything and the caller has to say `--all` to get one.
    pub fn is_empty(&self) -> bool {
        self.older_than.is_none() && self.country.is_none()
    }

    fn matches(&self, entry: &CacheEntry) -> bool {
        if let Some(cutoff) = self.older_than {
            if entry.modified >= cutoff {
                return false;
            }
        }
        if let Some(ref country) = self.country {
            if entry.country.as_deref() != Some(country.as_str()) {
                return false;
            }
        }
        true
    }
}

/// What `cache clear` did.
#[derive(Debug, Clone, Default)]
pub struct CacheClearReport {
    pub dir: PathBuf,
    pub removed: Vec<String>,
    pub removed_bytes: u64,
    pub kept: usize,
    /// Entries the filter could not decide about, with the reason.
    ///
    /// Only ever populated by `--country`, and only ever with search entries.
    /// Reported rather than silently skipped: "cleared the Norwegian cache"
    /// while leaving the Norwegian search results in place is the kind of
    /// half-truth a caller acts on.
    pub unattributable: usize,
    /// Entries the filter chose and the filesystem refused, with the reason.
    pub failed: Vec<String>,
}

impl Cache {
    /// Every cache file in the directory.
    ///
    /// **The only place either management command decides what a cache file
    /// is,** so `stats` counts exactly what `clear` can remove. A regular
    /// `.json` file sitting directly in the resolved directory: not a
    /// directory, not a symlink, and nothing one level down. A missing
    /// directory is an empty cache rather than a failure — that is the state a
    /// machine that has never run this tool is in.
    pub fn entries(&self) -> Result<Vec<CacheEntry>, IherbError> {
        let read = match std::fs::read_dir(&self.dir) {
            Ok(read) => read,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(IherbError::CacheUnreadable(format!(
                    "{}: {}",
                    self.dir.display(),
                    e
                )))
            }
        };

        let mut entries = Vec::new();
        for item in read {
            let item = match item {
                Ok(item) => item,
                Err(e) => {
                    tracing::warn!("Skipping an unreadable cache directory entry: {}", e);
                    continue;
                }
            };
            let path = item.path();

            // `symlink_metadata`, not `metadata`: a symlink in here points
            // somewhere else, and `clear` must never follow one out of the
            // directory it was told to work in.
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(e) => {
                    tracing::warn!("Skipping {}: {}", path.display(), e);
                    continue;
                }
            };
            if !meta.is_file() || meta.file_type().is_symlink() {
                continue;
            }
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            entries.push(CacheEntry {
                name: name.to_string(),
                bytes: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                country: CacheEntry::country_from_name(name),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    /// What the cache directory holds.
    pub fn stats(&self) -> Result<CacheStats, IherbError> {
        let entries = self.entries()?;
        Ok(CacheStats {
            dir: self.dir.clone(),
            entries: entries.len(),
            bytes: entries.iter().map(|e| e.bytes).sum(),
            oldest: entries.iter().map(|e| e.modified).min(),
            newest: entries.iter().map(|e| e.modified).max(),
        })
    }

    /// Remove the entries the filter chooses, and say what happened.
    ///
    /// Every path removed is `self.dir.join(entry.name)`, where `entry.name` is
    /// a single file component this cache itself enumerated — so a removal
    /// cannot address anything outside the directory even if a file in it is
    /// named strangely. Symlinks never reach here; [`Cache::entries`] drops
    /// them.
    pub fn clear(&self, filter: &ClearFilter) -> Result<CacheClearReport, IherbError> {
        let entries = self.entries()?;
        let mut report = CacheClearReport {
            dir: self.dir.clone(),
            ..Default::default()
        };

        for entry in entries {
            if filter.country.is_some() && entry.country.is_none() {
                report.unattributable += 1;
                report.kept += 1;
                continue;
            }
            if !filter.matches(&entry) {
                report.kept += 1;
                continue;
            }
            let path = self.dir.join(&entry.name);
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    report.removed.push(entry.name);
                    report.removed_bytes += entry.bytes;
                }
                Err(e) => {
                    report.kept += 1;
                    report.failed.push(format!("{}: {}", entry.name, e));
                }
            }
        }
        Ok(report)
    }
}
