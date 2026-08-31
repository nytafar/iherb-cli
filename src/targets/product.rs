//! A single product detail page.

use anyhow::{Context, Result};
use chromiumoxide::Page;

use crate::cache::CacheKey;
use crate::config::AppConfig;
use crate::error::IherbError;
use crate::fetch::{FetchTarget, Paging};
use crate::model::{ProductDetail, Source};
use crate::scraper;

pub struct ProductTarget {
    product_id: String,
    base_url: String,
    currency: String,
}

impl ProductTarget {
    /// Build a target from a numeric ID or a full iHerb URL.
    pub fn new(config: &AppConfig, id_or_url: &str) -> Result<Self> {
        Ok(Self {
            product_id: parse_product_identifier(id_or_url)?,
            base_url: config.base_url(),
            currency: config.currency.clone(),
        })
    }

    pub fn product_id(&self) -> &str {
        &self.product_id
    }
}

impl FetchTarget for ProductTarget {
    type Output = ProductDetail;
    type Accumulator = Option<ProductDetail>;

    fn cache_key(&self) -> CacheKey {
        CacheKey::Product {
            product_id: self.product_id.clone(),
        }
    }

    fn url(&self, _page_num: usize) -> String {
        format!("{}/pr/item/{}", self.base_url, self.product_id)
    }

    fn navigation_context(&self) -> String {
        "Failed to navigate to product page".to_string()
    }

    async fn extract<'a>(
        &'a self,
        page: &'a Page,
        html: &'a str,
        acc: &'a mut Self::Accumulator,
    ) -> Result<Paging> {
        // The one place `ProductNotFound` is right: the page itself says the
        // product is gone.
        if scraper::helpers::is_not_found_page(html) {
            return Err(IherbError::ProductNotFound(self.product_id.clone()).into());
        }

        let product = scraper::product::extract_product(
            page,
            html,
            &self.product_id,
            &self.base_url,
            &self.currency,
        )
        .await
        .context("Failed to extract product data")?;

        *acc = Some(product);
        Ok(Paging::Done)
    }

    fn finish(&self, acc: Self::Accumulator) -> Result<Self::Output> {
        // Nothing accumulated means no page was ever read into a record. That
        // is a broken pipeline, not a missing product.
        acc.ok_or_else(|| IherbError::ParseFailed(self.product_id.clone()).into())
    }

    /// Reject a record no strategy actually produced.
    ///
    /// This replaces a heuristic (#28). The old check asked whether the values
    /// looked like junk — an empty name, or a zero price with no rating and no
    /// review count — and called anything that failed it `product_not_found`.
    /// That is why a Cloudflare block reported as "product not found" (#23):
    /// the worst available answer, because it tells the caller to give up on an
    /// id that is perfectly valid.
    ///
    /// The question the check asks now is provenance, not shape: **did any
    /// strategy produce a name and a price?** A field that no strategy produced
    /// is [`Source::Absent`], and a record whose name or price is absent is a
    /// record extraction failed to build — `parse_failed`, which means "retry
    /// is reasonable and a human should look at the selectors".
    ///
    /// `product_not_found` is now reserved for a page that actually says the
    /// product is gone, and is raised in `extract` rather than here.
    fn validate(&self, product: &Self::Output) -> Result<()> {
        let missing: Vec<&str> = ["name", "price"]
            .into_iter()
            .filter(|f| product.source_of(f) == Source::Absent)
            .collect();

        if !missing.is_empty() {
            tracing::error!(
                "No strategy produced {} for product {}; reporting parse_failed",
                missing.join(" or "),
                self.product_id
            );
            return Err(IherbError::ParseFailed(self.product_id.clone()).into());
        }

        // A record that parsed but is missing something every product page
        // publishes is usable and suspect at the same time. It is not an error;
        // it is the signal that our selectors are rotting.
        let health = product.health();
        if health.degraded {
            tracing::warn!(
                "Product {} extracted via {:?} is degraded: {} absent",
                self.product_id,
                health.strategy,
                health.fields_absent.join(", ")
            );
        }

        Ok(())
    }
}

/// Accept either a bare numeric product ID or a full iHerb product URL.
pub fn parse_product_identifier(input: &str) -> Result<String> {
    if input.chars().all(|c| c.is_ascii_digit()) && !input.is_empty() {
        return Ok(input.to_string());
    }

    if input.contains("iherb.com") {
        if let Some(id) = input
            .split('/')
            .rev()
            .find(|s| s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty())
        {
            return Ok(id.to_string());
        }
    }

    anyhow::bail!(
        "Invalid product identifier: {}. Use a numeric ID or full iHerb URL",
        input
    );
}
