//! Application entry point: wires parsed CLI arguments through to the commands.
//!
//! Each command is now three steps — build a target, fetch it, format it. The
//! cache/launch/navigate/store sequence they used to each carry a copy of lives
//! in [`crate::fetch`].

use anyhow::{Context, Result};

use crate::browser::session::BrowserSession;
use crate::cli::{Cli, Commands, Section, SortOrder};
use crate::config::AppConfig;
use crate::fetch::fetch;
use crate::output;
use crate::targets::{ProductTarget, SearchTarget};

/// Run the CLI: configure logging, load config, and dispatch the subcommand.
pub async fn run(cli: Cli) -> Result<()> {
    let filter = if cli.debug {
        "iherb_cli=debug"
    } else {
        "iherb_cli=warn"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .with_target(false)
        .init();

    let config = AppConfig::load(
        cli.country,
        cli.currency,
        cli.no_cache,
        cli.delay,
        cli.debug,
    )?;

    ctrlc::set_handler(|| {
        eprintln!("\nInterrupted.");
        std::process::exit(130);
    })
    .context("Failed to set Ctrl+C handler")?;

    let mut browser_session: Option<BrowserSession> = None;

    match cli.command {
        Commands::Search {
            query,
            limit,
            sort,
            category,
        } => {
            cmd_search(
                &config,
                &mut browser_session,
                &query,
                limit,
                sort,
                category.as_deref(),
            )
            .await?;
        }
        Commands::Product { id_or_url, section } => {
            cmd_product(&config, &mut browser_session, &id_or_url, section).await?;
        }
    }

    if let Some(session) = browser_session.take() {
        if let Err(e) = session.close().await {
            tracing::warn!("Failed to close browser: {}", e);
        }
    }

    Ok(())
}

pub async fn cmd_search(
    config: &AppConfig,
    browser_session: &mut Option<BrowserSession>,
    query: &str,
    limit: usize,
    sort: SortOrder,
    category: Option<&str>,
) -> Result<()> {
    let target = SearchTarget::new(config, query, limit, sort, category)?;
    let fetched = fetch(&target, config, browser_session).await?;

    // The cache holds every product that was fetched; only the display is
    // capped at --limit.
    let mut result = fetched.data;
    result.products.truncate(limit);

    print!("{}", output::format_search_results(&result));
    println!(
        "\n- **Data from:** {}",
        output::format_cached_at(fetched.retrieved_at)
    );
    Ok(())
}

pub async fn cmd_product(
    config: &AppConfig,
    browser_session: &mut Option<BrowserSession>,
    id_or_url: &str,
    section: Option<Section>,
) -> Result<()> {
    let target = ProductTarget::new(config, id_or_url)?;
    let fetched = fetch(&target, config, browser_session).await?;

    print!("{}", output::format_product_detail(&fetched.data, section));

    // The provenance table is reachable from a caller, on demand. #9 renders
    // the same data as a machine-readable block under `--json`; see
    // `output::format_extraction_health` for exactly what it has to emit.
    if config.debug {
        print!(
            "\n{}",
            output::format_extraction_health(&fetched.data.health())
        );
    }

    println!(
        "\n- **Data from:** {}",
        output::format_cached_at(fetched.retrieved_at)
    );
    Ok(())
}
