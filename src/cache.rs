use crate::cli::SortOrder;
use crate::error::IherbError;
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Where a fetched artefact lives in the cache.
///
/// One value per kind of cacheable thing, so a new command declares its cache
/// identity instead of adding another pair of `get_x`/`set_x` methods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheKey {
    Product {
        product_id: String,
    },
    Search {
        query: String,
        sort: SortOrder,
        category: Option<String>,
    },
}

impl CacheKey {
    /// The cache file name. The derivation is fixed: changing it orphans every
    /// entry users already have on disk.
    fn file_name(&self) -> String {
        match self {
            CacheKey::Product { product_id } => format!("product_{}.json", product_id),
            CacheKey::Search {
                query,
                sort,
                category,
            } => {
                let mut hasher = Sha256::new();
                hasher.update(query.as_bytes());
                hasher.update(b"\0");
                hasher.update(sort.as_cache_key().as_bytes());
                hasher.update(b"\0");
                if let Some(cat) = category {
                    hasher.update(cat.as_bytes());
                }
                let result = hasher.finalize();
                format!("search_{}.json", hex::encode(&result[..8])) // 16 hex chars
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
