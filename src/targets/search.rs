//! A search result set, walked one result page at a time until `limit` products
//! have been gathered or the results run out.

use anyhow::{Context, Result};
use chromiumoxide::Page;

use crate::cache::CacheKey;
use crate::cli::SortOrder;
use crate::config::AppConfig;
use crate::fetch::{FetchTarget, Paging};
use crate::model::{ProductSummary, SearchResult};
use crate::scraper;
use crate::scraper::search::CategoryId;

pub struct SearchTarget {
    query: String,
    limit: usize,
    sort: SortOrder,
    category: Option<CategoryId>,
    base_url: String,
    currency: String,
}

/// Products gathered so far, plus the result total from the first page that
/// reported one.
#[derive(Default)]
pub struct SearchPages {
    products: Vec<ProductSummary>,
    total_results: Option<u32>,
}

impl SearchTarget {
    pub fn new(
        config: &AppConfig,
        query: &str,
        limit: usize,
        sort: SortOrder,
        category: Option<&str>,
    ) -> Result<Self> {
        if query.trim().is_empty() {
            anyhow::bail!("Search query cannot be empty");
        }
        if limit == 0 {
            anyhow::bail!("Limit must be at least 1");
        }

        // Resolved here rather than at the URL builder so an unusable
        // `--category` fails before anything launches a browser, and so the
        // cache key names the id the request will actually carry: `supplements`
        // and `1855` are the same fetch and share an entry (#4).
        let category = category.map(CategoryId::resolve).transpose()?;

        Ok(Self {
            query: query.to_string(),
            limit,
            sort,
            category,
            base_url: config.base_url(),
            currency: config.currency.clone(),
        })
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn limit(&self) -> usize {
        self.limit
    }
}

impl FetchTarget for SearchTarget {
    type Output = SearchResult;
    type Accumulator = SearchPages;

    fn cache_key(&self) -> CacheKey {
        CacheKey::Search {
            query: self.query.clone(),
            sort: self.sort,
            category: self.category.as_ref().map(|c| c.as_str().to_string()),
        }
    }

    fn url(&self, page_num: usize) -> String {
        scraper::search::build_search_url(
            &self.base_url,
            &self.query,
            self.sort,
            self.category.as_ref(),
            page_num,
        )
    }

    fn page_count(&self) -> usize {
        scraper::search::pages_needed(self.limit)
    }

    fn navigation_context(&self) -> String {
        "Failed to navigate to search page".to_string()
    }

    fn has_enough(&self, acc: &Self::Accumulator) -> bool {
        acc.products.len() >= self.limit
    }

    async fn extract<'a>(
        &'a self,
        page: &'a Page,
        html: &'a str,
        acc: &'a mut Self::Accumulator,
    ) -> Result<Paging> {
        let page_result = scraper::search::extract_search(
            page,
            html,
            &self.query,
            &self.base_url,
            &self.currency,
        )
        .await
        .context("Failed to extract search results")?;

        if page_result.products.is_empty() {
            return Ok(Paging::Done);
        }

        if acc.total_results.is_none() {
            acc.total_results = page_result.total_results;
        }
        acc.products.extend(page_result.products);

        Ok(Paging::More)
    }

    /// Assemble the *full* result set. Truncation to `limit` is the caller's
    /// job, so the cache keeps everything that was fetched.
    fn finish(&self, acc: Self::Accumulator) -> Result<Self::Output> {
        Ok(SearchResult {
            query: self.query.clone(),
            total_results: acc.total_results,
            products: acc.products,
        })
    }

    fn validate(&self, result: &Self::Output) -> Result<()> {
        if result.products.is_empty() {
            anyhow::bail!("No search results found for: {}", self.query);
        }
        Ok(())
    }
}
