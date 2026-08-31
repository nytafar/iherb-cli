use thiserror::Error;

#[derive(Error, Debug)]
pub enum IherbError {
    #[error("Failed to launch browser: {0}")]
    BrowserLaunch(String),

    #[error("Browser navigation failed: {0}")]
    Navigation(String),

    #[error("Cloudflare challenge could not be solved after {0} attempts")]
    CloudflareBlocked(u32),

    /// The page said the product is gone: a 404, or a not-found page. Reserved
    /// for that — a caller seeing this should stop asking about the id.
    #[error("Product not found: {0}")]
    ProductNotFound(String),

    /// The page loaded, was not a 404, and no extraction strategy produced
    /// usable data from it.
    ///
    /// Distinct from [`IherbError::ProductNotFound`] on purpose (#28). This one
    /// means the scraper is broken or the site changed shape — retrying the
    /// same id is reasonable, and a human should look. Reporting it as
    /// "not found" is the worst available misclassification, because it tells
    /// the caller to give up on an id that is fine.
    #[error("Extraction failed for product {0}: the page loaded but no strategy produced usable data. The scraper may need updating.")]
    ParseFailed(String),

    #[error("Chrome download failed: {0}")]
    ChromeDownload(String),

    #[error("Cache error: {0}")]
    Cache(String),

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
