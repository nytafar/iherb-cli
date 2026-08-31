//! A search result set, walked one result page at a time until `limit` products
//! have been gathered or the results run out.

use anyhow::{Context, Result};
use chromiumoxide::Page;
use std::collections::HashSet;

use crate::cache::CacheKey;
use crate::cli::SortOrder;
use crate::config::AppConfig;
use crate::fetch::{FetchTarget, Paging};
use crate::model::{ProductSummary, SearchFetch, SearchResult};
use crate::scraper;
use crate::scraper::search::CategoryId;

pub struct SearchTarget {
    query: String,
    limit: usize,
    sort: SortOrder,
    category: Option<CategoryId>,
    country: String,
    base_url: String,
    currency: String,
}

/// Products gathered so far, plus the result total from the first page that
/// reported one.
#[derive(Default)]
pub struct SearchPages {
    products: Vec<ProductSummary>,
    total_results: Option<u32>,
    /// The product ids already gathered, so a product promoted onto one page
    /// and listed again on the next is one product (#33). Deduplicating each
    /// page in isolation would not catch that.
    seen: HashSet<String>,
    /// Result pages walked so far.
    pages_fetched: usize,
    /// Whether a page came back with no products, i.e. iHerb ran out.
    exhausted: bool,
}

impl SearchPages {
    /// How many distinct products have been gathered.
    pub fn gathered(&self) -> usize {
        self.products.len()
    }

    /// Result pages walked so far.
    pub fn pages_fetched(&self) -> usize {
        self.pages_fetched
    }
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
            country: config.country.clone(),
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

    /// Fold one parsed result page into what has been gathered, and say whether
    /// to ask for another.
    ///
    /// Split out of [`FetchTarget::extract`] so the paging rules — cross-page
    /// deduplication (#33) and how a walk ends (#6) — can be exercised without
    /// a browser. `extract` is this plus the parse.
    pub fn absorb(&self, page_result: SearchResult, acc: &mut SearchPages) -> Paging {
        acc.pages_fetched += 1;

        // A page with no cards is iHerb saying there is nothing after this,
        // which is the one signal that distinguishes "we have them all" from
        // "we stopped early". #6 depends on the difference.
        if page_result.products.is_empty() {
            acc.exhausted = true;
            return Paging::Done;
        }

        if acc.total_results.is_none() {
            acc.total_results = page_result.total_results;
        }
        acc.products.extend(scraper::search::retain_first_seen(
            page_result.products,
            &mut acc.seen,
        ));

        Paging::More
    }
}

impl FetchTarget for SearchTarget {
    type Output = SearchResult;
    type Accumulator = SearchPages;

    fn cache_key(&self) -> CacheKey {
        CacheKey::Search {
            country: self.country.clone(),
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
        scraper::search::page_budget(self.limit)
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

        Ok(self.absorb(page_result, acc))
    }

    /// Assemble the *full* result set. Truncation to `limit` is the caller's
    /// job, so the cache keeps everything that was fetched.
    fn finish(&self, acc: Self::Accumulator) -> Result<Self::Output> {
        Ok(SearchResult {
            query: self.query.clone(),
            total_results: acc.total_results,
            products: acc.products,
            // What this walk did, so a later, wider request reading it back can
            // tell a record that is short because iHerb has no more from one
            // that is short because this run did not ask for more (#6).
            fetch: SearchFetch {
                pages_fetched: Some(acc.pages_fetched),
                exhausted: Some(acc.exhausted),
            },
        })
    }

    /// A cached result set answers this request when it holds as many distinct
    /// products as `--limit` asked for, or when the run that wrote it walked to
    /// the end of iHerb's results and there are no more to be had.
    ///
    /// A record that says nothing about its walk — one written before that was
    /// recorded — is not treated as complete. Assuming it was is how #6 read to
    /// a caller: silently fewer results than asked for, with a plausible
    /// timestamp and a header still quoting the full total.
    fn cache_is_sufficient(&self, cached: &Self::Output) -> bool {
        cached.products.len() >= self.limit || cached.fetch.exhausted == Some(true)
    }

    fn validate(&self, result: &Self::Output) -> Result<()> {
        if result.products.is_empty() {
            anyhow::bail!("No search results found for: {}", self.query);
        }
        Ok(())
    }
}
