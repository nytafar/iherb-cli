//! Application entry point: wires parsed CLI arguments through to the commands.
//!
//! Each command is now three steps — build a target, fetch it, format it. The
//! cache/launch/navigate/store sequence they used to each carry a copy of lives
//! in [`crate::fetch`].

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::Notify;

use crate::browser::session::BrowserSession;
use crate::cli::{Cli, Commands, Section, SortOrder};
use crate::config::AppConfig;
use crate::fetch::fetch;
use crate::output;
use crate::targets::{ProductTarget, SearchTarget};

/// The exit status a process killed by SIGINT reports: 128 + SIGINT.
const EXIT_INTERRUPTED: i32 = 130;

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

    // The handler used to call `process::exit(130)`, which runs no destructors:
    // the browser handle was never dropped, so Chrome was orphaned along with
    // its temporary profile — 9 processes and 25 MB, measured (#46). It now
    // only *asks* for shutdown, and the asking happens on the main task where
    // the browser can actually be closed.
    //
    // Impatience is still honoured, in two steps. A second interrupt gives up
    // on the graceful close and kills the browser; a third exits outright.
    // Someone pressing Ctrl+C repeatedly is saying the polite path is not
    // getting there, and a CLI that cannot be stopped is worse than a leak.
    let interrupted = Arc::new(Notify::new());
    let impatient = Arc::new(Notify::new());
    {
        let interrupted = Arc::clone(&interrupted);
        let impatient = Arc::clone(&impatient);
        let presses = AtomicUsize::new(0);
        ctrlc::set_handler(move || {
            // Two separate notifications rather than one counted twice: a
            // `Notify` holds at most one permit, so a double tap during the
            // command would otherwise arrive as a single request and cancel the
            // shutdown it had just asked for.
            match presses.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    eprintln!("\nInterrupted. Closing the browser...");
                    // `notify_one`, not `notify_waiters`: the signal can arrive
                    // before anything is waiting, and a permit that is stored
                    // is a shutdown that happens.
                    interrupted.notify_one();
                }
                1 => {
                    eprintln!("\nInterrupted again. Killing the browser.");
                    impatient.notify_one();
                }
                _ => std::process::exit(EXIT_INTERRUPTED),
            }
        })
        .context("Failed to set Ctrl+C handler")?;
    }

    let mut browser_session: Option<BrowserSession> = None;

    // The command's future borrows `browser_session`, so it has to be dropped
    // before the shutdown below can take the session out of it. That is what
    // the block is for.
    let outcome = {
        let command = dispatch(cli.command, &config, &mut browser_session);
        tokio::pin!(command);

        tokio::select! {
            result = &mut command => Outcome::Ran(result),
            _ = interrupted.notified() => Outcome::Interrupted,
        }
    };

    // Reached on every path out of the command — success, error and interrupt
    // alike. It used to sit after a `?`, so any failing fetch skipped it and
    // left its profile directory behind.
    //
    // A second Ctrl+C abandons the graceful close, and abandoning it is not the
    // same as skipping it: dropping the `close` future drops the session inside
    // it, and *that* is what kills Chrome and removes the profile directory.
    // The old handler's `process::exit` is the one thing that reaches neither.
    if let Some(session) = browser_session.take() {
        tokio::select! {
            result = session.close() => {
                if let Err(e) = result {
                    tracing::warn!("Failed to close browser: {}", e);
                }
            }
            _ = impatient.notified() => {
                // Nothing to do here, and that is the fix rather than an
                // omission. Abandoning the graceful close drops the future that
                // owns the session, and dropping a session kills Chrome and
                // then removes its profile directory, waiting for Chrome to let
                // go if the first attempt finds it still holding on.
                //
                // This arm used to sleep and then call `remove_dir_all` itself,
                // because `Drop` at the time got a single bare attempt that
                // usually lost the race. It no longer does, so a second removal
                // here would only run *before* Chrome had been killed and
                // achieve nothing. Bounded either way: a third Ctrl+C exits
                // outright.
            }
        }
    }

    match outcome {
        Outcome::Ran(result) => result,
        // Exiting here skips no cleanup: the browser is already closed and its
        // profile directory already gone. What it buys is the 130 a caller
        // reads to tell an interrupt from a failure.
        Outcome::Interrupted => std::process::exit(EXIT_INTERRUPTED),
    }
}

/// How [`run`]'s command ended.
enum Outcome {
    /// The command finished on its own, well or badly.
    Ran(Result<()>),
    /// Ctrl+C arrived first and the command was dropped where it stood.
    Interrupted,
}

/// Run the subcommand the user asked for.
///
/// Separate from [`run`] so there is a single future to race against the
/// interrupt, and so that the browser shutdown is not something a `?` in here
/// can jump over.
async fn dispatch(
    command: Commands,
    config: &AppConfig,
    browser_session: &mut Option<BrowserSession>,
) -> Result<()> {
    match command {
        Commands::Search {
            query,
            limit,
            sort,
            category,
        } => {
            cmd_search(
                config,
                browser_session,
                &query,
                limit,
                sort,
                category.as_deref(),
            )
            .await
        }
        Commands::Product { id_or_url, section } => {
            cmd_product(config, browser_session, &id_or_url, section).await
        }
    }
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

    // `--limit` counts distinct products, so falling short of it is a fact
    // about the fetch and has to be said out loud (#6, #33). A caller counting
    // rows cannot otherwise tell "iHerb has no more" from "we stopped walking".
    if let Some(note) = output::format_search_shortfall(&result, limit) {
        print!("\n{}", note);
    }

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
