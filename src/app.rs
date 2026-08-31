//! Application entry point: wires parsed CLI arguments through to the commands.
//!
//! Each command is now three steps — build a target, fetch it, format it. The
//! cache/launch/navigate/store sequence they used to each carry a copy of lives
//! in [`crate::fetch`].
//!
//! A command no longer prints. It returns a [`CommandOutcome`] — what was
//! fetched, and the view of it the flags asked for — and [`run`] renders that
//! once, as Markdown or as one JSON document (#9). The commands used to
//! `print!` directly, which meant the only way to add a second rendering was to
//! add a second set of print statements beside the first; the fact that a
//! product record and a search result are two values, not two piles of output,
//! is what makes one envelope possible over both (#44).

use std::ffi::OsStr;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use tokio::sync::Notify;

use crate::browser::session::BrowserSession;
use crate::cli::{Cli, Commands, Section, SortOrder};
use crate::config::AppConfig;
use crate::error::{classify_error, ErrorKind};
use crate::fetch::fetch;
use crate::model::{ProductDetail, SearchResult};
use crate::output::{self, Envelope, Meta, ProductView, Provenance};
use crate::targets::{ProductTarget, SearchTarget};

/// The exit status a process killed by SIGINT reports: 128 + SIGINT.
const EXIT_INTERRUPTED: u8 = 130;

/// Run the CLI: configure logging, load config, dispatch the subcommand, and
/// render whatever came back.
///
/// Returns the process exit code rather than a `Result`, because the code *is*
/// the result as far as a caller is concerned (#9): `1` for everything told a
/// script nothing it could act on, and the four failures that need four
/// different responses — skip the id, retry later, fix the environment, file a
/// bug — were indistinguishable.
pub async fn run(cli: Cli) -> ExitCode {
    let json = cli.json;

    let filter = if cli.debug {
        "iherb_cli=debug"
    } else {
        "iherb_cli=warn"
    };
    tracing_subscriber::fmt()
        // Unconditionally, not only under `--json`. A warning has never
        // belonged on stdout: the subscriber defaulted there, so a cache-parse
        // warning landed in the middle of the Markdown a caller was reading,
        // and would land in the middle of a JSON document. `--json` is what
        // made it unignorable, not what made it wrong.
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .with_target(false)
        .init();

    let config = match AppConfig::load(
        cli.country,
        cli.currency,
        cli.no_cache,
        cli.delay,
        cli.debug,
    ) {
        Ok(config) => config,
        // No config, so no storefront to name in the envelope. Reported
        // honestly as nulls rather than as a guess — see [`Meta`].
        Err(e) => return report_failure(None, e.into(), json),
    };

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
        let handler = ctrlc::set_handler(move || {
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
                _ => std::process::exit(EXIT_INTERRUPTED as i32),
            }
        });
        if let Err(e) = handler {
            return report_failure(
                Some(&config),
                anyhow::Error::new(e).context("Failed to set Ctrl+C handler"),
                json,
            );
        }
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
        Outcome::Ran(Ok(command)) => {
            print!("{}", render(&command, &config, json));
            ExitCode::SUCCESS
        }
        Outcome::Ran(Err(e)) => report_failure(Some(&config), e, json),
        // Returning here skips no cleanup: the browser is already closed and
        // its profile directory already gone. What it buys is the 130 a caller
        // reads to tell an interrupt from a failure.
        Outcome::Interrupted => ExitCode::from(EXIT_INTERRUPTED),
    }
}

/// How [`run`]'s command ended.
enum Outcome {
    /// The command finished on its own, well or badly.
    Ran(Result<CommandOutcome>),
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
) -> Result<CommandOutcome> {
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
) -> Result<CommandOutcome> {
    let target = SearchTarget::new(config, query, limit, sort, category)?;
    let fetched = fetch(&target, config, browser_session).await?;
    let provenance = Provenance::from(&fetched);

    // The cache holds every product that was fetched; only the display is
    // capped at --limit.
    let mut result = fetched.data;
    result.products.truncate(limit);

    Ok(CommandOutcome::Search {
        result,
        limit,
        provenance,
    })
}

pub async fn cmd_product(
    config: &AppConfig,
    browser_session: &mut Option<BrowserSession>,
    id_or_url: &str,
    section: Option<Section>,
) -> Result<CommandOutcome> {
    let target = ProductTarget::new(config, id_or_url)?;
    let fetched = fetch(&target, config, browser_session).await?;

    Ok(CommandOutcome::Product {
        provenance: Provenance::from(&fetched),
        product: Box::new(fetched.data),
        // Resolved here, once, from the flag. Both renderings below read the
        // answer; neither re-derives it. See [`ProductView`].
        view: ProductView::for_section(section),
    })
}

/// What a command produced, before anything decides how to show it.
///
/// The value and the provenance travel together because the envelope needs
/// both, and because they are only correct together: `fetched_at` describes
/// *this* record and nothing else can supply it once the record is on its own.
pub enum CommandOutcome {
    Search {
        result: SearchResult,
        /// What `--limit` asked for, which a short result has to be measured
        /// against (#6).
        limit: usize,
        provenance: Provenance,
    },
    Product {
        /// Boxed because a `ProductDetail` is an order of magnitude larger than
        /// a `SearchResult`, and an enum is as big as its widest variant.
        product: Box<ProductDetail>,
        view: ProductView,
        provenance: Provenance,
    },
}

impl CommandOutcome {
    fn provenance(&self) -> Provenance {
        match self {
            CommandOutcome::Search { provenance, .. } => *provenance,
            CommandOutcome::Product { provenance, .. } => *provenance,
        }
    }
}

/// Render a finished command, as Markdown or as one JSON document.
fn render(outcome: &CommandOutcome, config: &AppConfig, json: bool) -> String {
    if json {
        render_json(outcome, config, SystemTime::now())
    } else {
        render_markdown(outcome, config)
    }
}

/// The Markdown a human — or an agent reading prose — gets.
fn render_markdown(outcome: &CommandOutcome, config: &AppConfig) -> String {
    let mut out = String::new();

    match outcome {
        CommandOutcome::Search { result, limit, .. } => {
            out.push_str(&output::format_search_results(result));

            // `--limit` counts distinct products, so falling short of it is a
            // fact about the fetch and has to be said out loud (#6, #33). A
            // caller counting rows cannot otherwise tell "iHerb has no more"
            // from "we stopped walking".
            if let Some(note) = output::format_search_shortfall(result, *limit) {
                out.push_str(&format!("\n{}", note));
            }
        }
        CommandOutcome::Product { product, view, .. } => {
            out.push_str(&output::format_product_detail(product, view));

            // The provenance table is reachable from a caller, on demand.
            // `--json` carries the same block unconditionally, because there it
            // costs a consumer nothing to ignore and costs it everything to be
            // unable to ask.
            if config.debug {
                out.push_str(&format!(
                    "\n{}",
                    output::format_extraction_health(&product.health())
                ));
            }
        }
    }

    out.push_str(&format!(
        "\n- **Data from:** {}\n",
        output::format_cached_at(outcome.provenance().fetched_at)
    ));
    out
}

/// The single JSON document a machine gets: one envelope, whatever the command.
///
/// `emitted_at` is a parameter rather than a call to the clock so that the
/// document is a function of its inputs — which is what makes the cached-versus-
/// fresh distinction in `meta` assertable at all.
fn render_json(outcome: &CommandOutcome, config: &AppConfig, emitted_at: SystemTime) -> String {
    let meta = Meta::new(config, Some(outcome.provenance()), emitted_at);

    let data = match outcome {
        CommandOutcome::Search { result, .. } => output::format_search_json(result),
        CommandOutcome::Product { product, view, .. } => output::format_product_json(product, view),
    };

    match data {
        Ok(data) => Envelope::success(meta, data).render(),
        // Serializing a record we already hold cannot fail in practice, but
        // "cannot fail" is not "need not be answered": under `--json` there is
        // exactly one document on stdout, and a panic here would emit none.
        Err(e) => Envelope::failure(meta, ErrorKind::JsonError, e.to_string()).render(),
    }
}

/// Report a failed run and produce its exit code.
///
/// `config` is `None` when the failure happened before the configuration
/// resolved. Without it the envelope's storefront fields are `null`, which is
/// the truth: an unparseable command line has no effective storefront.
fn report_failure(config: Option<&AppConfig>, error: anyhow::Error, json: bool) -> ExitCode {
    let kind = classify_error(&error);

    if json {
        let emitted_at = SystemTime::now();
        let meta = match config {
            // No provenance: a failure means no page was read, and `null` says
            // so rather than dating the failure as if it were data.
            Some(config) => Meta::new(config, None, emitted_at),
            None => Meta::unconfigured(None, emitted_at),
        };
        // `{:#}` rather than `{:?}`: the whole anyhow chain on one line, which
        // is what a JSON string wants, instead of the multi-line debug report
        // the human path prints.
        print!(
            "{}",
            Envelope::failure(meta, kind, format!("{:#}", error)).render()
        );
    } else {
        eprintln!("Error: {:?}", error);
    }

    ExitCode::from(kind.exit_code())
}

/// Whether the caller asked for JSON, read straight off `argv`.
///
/// Clap fails *before* the parsed struct exists, so a command line clap will
/// reject cannot be asked whether it wanted JSON — and a parse error that
/// answers in Markdown breaks the one promise `--json` makes, which is that
/// stdout carries exactly one JSON document whatever happens.
///
/// A bare `--` ends the search: everything after it is a value by definition,
/// so a product whose id is literally `--json` is not a request for JSON.
pub fn wants_json<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    for arg in args {
        let arg = arg.as_ref();
        if arg == "--" {
            return false;
        }
        if arg == "--json" {
            return true;
        }
    }
    false
}

/// Report a command line clap refused, honouring `--json`.
///
/// `--help` and `--version` are not errors and are not command output: they
/// print as they always have and exit 0, `--json` or not. Wrapping a usage
/// message in an envelope would give a caller a JSON document containing help
/// text, which is of no use to the machine and worse for the human.
pub fn report_clap_error(error: clap::Error, json: bool) -> ExitCode {
    if !json || !error.use_stderr() {
        // Prints and exits: 0 for help and version, 2 for a real parse error,
        // exactly as `Cli::parse()` did.
        error.exit();
    }

    // `render()` rather than `to_string()`: the `Display` impl writes ANSI
    // escapes, and a colour code inside a JSON string is a message no consumer
    // can read.
    let message = error.render().to_string();
    let envelope = Envelope::failure(
        Meta::unconfigured(None, SystemTime::now()),
        ErrorKind::InvalidInput,
        message.trim_end().to_string(),
    );
    print!("{}", envelope.render());
    ExitCode::from(ErrorKind::InvalidInput.exit_code())
}
