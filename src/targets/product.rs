//! A single product detail page.

use anyhow::{Context, Result};
use chromiumoxide::Page;

use crate::cache::CacheKey;
use crate::config::AppConfig;
use crate::fetch::{FetchTarget, Paging};
use crate::model::ProductDetail;
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
        if scraper::helpers::is_not_found_page(html) {
            anyhow::bail!("Product not found: {}", self.product_id);
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
        acc.ok_or_else(|| anyhow::anyhow!("Product not found: {}", self.product_id))
    }

    /// Catch nonexistent product pages that slip through extraction (e.g. iHerb
    /// returns a page that doesn't trigger 404 detection but has no real
    /// product data).
    fn validate(&self, product: &Self::Output) -> Result<()> {
        if product.name.is_empty()
            || product.name == "Unknown Product"
            || (product.price == 0.0 && product.rating.is_none() && product.review_count.is_none())
        {
            anyhow::bail!("Product not found: {}", self.product_id);
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
