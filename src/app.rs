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
use serde_json::Value;
use tokio::sync::Notify;

use crate::browser::session::BrowserSession;
use crate::cache::{Cache, CacheClearReport, CacheStats, ClearFilter};
use crate::cli::{CacheCommand, Cli, Commands, Section, SortOrder};
use crate::config::{parse_duration, AppConfig};
use crate::error::{classify_error, ErrorKind, IherbError};
use crate::fetch::{fetch, Failure, Provenance};
use crate::model::{ProductDetail, SearchResult};
use crate::output::{self, Envelope, Freshness, Meta, ProductView};
use crate::targets::{ProductTarget, SearchTarget};

/// Run the CLI: configure logging, load config, dispatch the subcommand, and
/// render whatever came back.
///
/// Returns the process exit code rather than a `Result`, because the code *is*
/// the result as far as a caller is concerned (#9): `1` for everything told a
/// script nothing it could act on, and the four failures that need four
/// different responses — skip the id, retry later, fix the environment, file a
/// bug — were indistinguishable.
pub async fn run(cli: Cli) -> ExitCode {
    let json = cli.global.json;

    let filter = if cli.global.debug {
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

    let config = match AppConfig::load(&cli.global) {
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
                // The same 130 the polite path leaves on, from the same
                // place, so the impatient exit and the graceful one cannot
                // drift apart.
                _ => std::process::exit(i32::from(ErrorKind::Interrupted.exit_code())),
            }
        });
        if let Err(e) = handler {
            return report_failure(
                Some(&config),
                anyhow::Error::new(e)
                    .context("Failed to set Ctrl+C handler")
                    .into(),
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
            // One clock sample for the whole document, taken here: it dates the
            // run *and*, for a fresh record, the page the run read. See
            // [`crate::fetch::Provenance`].
            let (document, code) = render(&command, &config, json, SystemTime::now());
            print!("{}", document);
            ExitCode::from(code)
        }
        Outcome::Ran(Err(failure)) => report_failure(Some(&config), failure, json),
        // Returning here skips no cleanup: the browser is already closed and
        // its profile directory already gone (#46). What it buys is the 130 a
        // caller reads to tell an interrupt from a failure — and, since #9, a
        // document to read it out of.
        Outcome::Interrupted => report_interrupt(Some(&config), json),
    }
}

/// How [`run`]'s command ended.
enum Outcome {
    /// The command finished on its own, well or badly.
    Ran(Result<CommandOutcome, Failure>),
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
) -> Result<CommandOutcome, Failure> {
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
        // No browser, no network, no page. `cache` is file operations over the
        // directory `config` resolved, and the failure it can produce has
        // nothing to do with a fetch — so it carries no provenance, and the
        // envelope's `fetched_at` is `null` because no page was read.
        Commands::Cache { action } => cmd_cache(config, action).map_err(Failure::from),
    }
}

/// Inspect or manage the cache directory (#22).
pub fn cmd_cache(
    config: &AppConfig,
    action: CacheCommand,
) -> Result<CommandOutcome, anyhow::Error> {
    let cache = Cache::new(
        config.cache_dir.clone(),
        config.cache_mode,
        config.cache_ttl,
    );

    let report = match action {
        CacheCommand::Path => CacheReport::Path {
            dir: cache.dir().to_path_buf(),
        },
        CacheCommand::Stats => CacheReport::Stats(cache.stats()?),
        CacheCommand::Clear {
            older_than,
            country,
            all,
        } => {
            let filter = ClearFilter {
                older_than: match older_than.as_deref() {
                    Some(text) => Some(
                        SystemTime::now()
                            .checked_sub(parse_duration(text)?)
                            .ok_or_else(|| {
                                IherbError::InvalidInput(format!(
                                    "'{}' is further back than the clock goes.",
                                    text
                                ))
                            })?,
                    ),
                    None => None,
                },
                country: match country {
                    Some(country) => {
                        let country = country.trim().to_lowercase();
                        AppConfig::validate_country(&country)?;
                        Some(country)
                    }
                    None => None,
                },
            };

            // An unfiltered clear removes the whole cache, so it has to be
            // asked for. A prompt would be the other way to do it and is the
            // wrong one here: this tool is run by agents that cannot answer
            // one, and a prompt they cannot answer is a hang rather than a
            // safeguard.
            if filter.is_empty() && !all {
                return Err(IherbError::InvalidInput(
                    "`cache clear` with no --older-than and no --country removes every \
                     entry. Say --all if that is what you want."
                        .to_string(),
                )
                .into());
            }

            CacheReport::Cleared(cache.clear(&filter)?)
        }
    };

    Ok(CommandOutcome::Cache {
        report: Box::new(report),
    })
}

pub async fn cmd_search(
    config: &AppConfig,
    browser_session: &mut Option<BrowserSession>,
    query: &str,
    limit: usize,
    sort: SortOrder,
    category: Option<&str>,
) -> Result<CommandOutcome, Failure> {
    let target = SearchTarget::new(config, query, limit, sort, category)?;
    let fetched = fetch(&target, config, browser_session).await?;
    let provenance = fetched.provenance;

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
) -> Result<CommandOutcome, Failure> {
    let target = ProductTarget::new(config, id_or_url)?;
    let fetched = fetch(&target, config, browser_session).await?;

    Ok(CommandOutcome::Product {
        provenance: fetched.provenance,
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
    /// A `cache` invocation. Boxed for the same reason the product record is:
    /// an enum is as wide as its widest variant, and a clear report carries a
    /// list of file names.
    Cache { report: Box<CacheReport> },
}

/// What a `cache` invocation produced.
pub enum CacheReport {
    Path { dir: std::path::PathBuf },
    Stats(CacheStats),
    Cleared(CacheClearReport),
}

impl CommandOutcome {
    /// When and where this outcome's data was read, for the outcomes that read
    /// a page.
    ///
    /// `None` for `cache`, which reads no page: the envelope's `fetched_at` and
    /// `from_cache` are `null` there, which is what they already mean
    /// everywhere else — no page was read. Dating a `cache stats` document as
    /// though it had scraped something would be the same fabrication the
    /// `Data from:` bullet used to commit (#7, #44).
    fn provenance(&self) -> Option<Provenance> {
        match self {
            CommandOutcome::Search { provenance, .. } => Some(*provenance),
            CommandOutcome::Product { provenance, .. } => Some(*provenance),
            CommandOutcome::Cache { .. } => None,
        }
    }
}

/// Render a finished command, as Markdown or as one JSON document, and say what
/// the process should exit on.
///
/// The exit code comes back with the document because it is a property *of* the
/// document: `--json` can produce an envelope reporting a failure even when the
/// command succeeded, and a failure envelope under a success code is the one
/// combination a caller cannot recover from. See [`json_document`].
fn render(
    outcome: &CommandOutcome,
    config: &AppConfig,
    json: bool,
    emitted_at: SystemTime,
) -> (String, u8) {
    if json {
        render_json(outcome, config, emitted_at)
    } else {
        (render_markdown(outcome, config, emitted_at), 0)
    }
}

/// The Markdown a human — or an agent reading prose — gets.
///
/// This function no longer appends anything after the formatter has run, and
/// that is the whole of #7's first half. It used to `push_str` a
/// `- **Data from:**` bullet here, outside every section — which under
/// `--section` produced a section block followed by a top-level bullet
/// belonging to nothing, and under a section with no data a bullet under no
/// heading at all. Where the line belongs is a layout decision; layout is what
/// the formatter is for. See [`Freshness`].
fn render_markdown(outcome: &CommandOutcome, config: &AppConfig, emitted_at: SystemTime) -> String {
    match outcome {
        CommandOutcome::Search { result, limit, .. } => output::format_search_document(
            result,
            *limit,
            Freshness::of(
                outcome.provenance().expect("a search read a page"),
                emitted_at,
            ),
        ),
        CommandOutcome::Product { product, view, .. } => output::format_product_document(
            product,
            view,
            Freshness::of(
                outcome.provenance().expect("a product read a page"),
                emitted_at,
            ),
            config.debug,
        ),
        // No freshness footer: a `cache` document describes the cache
        // directory as it is right now, and there is no fetch for it to date.
        CommandOutcome::Cache { report } => output::format_cache_report(report, config),
    }
}

/// The single JSON document a machine gets: one envelope, whatever the command.
///
/// `emitted_at` is a parameter rather than a call to the clock so that the
/// document is a function of its inputs — which is what makes the cached-versus-
/// fresh distinction in `meta` assertable at all, and what lets one sample date
/// both the run and the page a fresh run read (#44).
fn render_json(
    outcome: &CommandOutcome,
    config: &AppConfig,
    emitted_at: SystemTime,
) -> (String, u8) {
    let meta = Meta::new(config, outcome.provenance(), emitted_at);

    let data = match outcome {
        CommandOutcome::Search { result, .. } => output::format_search_json(result),
        CommandOutcome::Product { product, view, .. } => output::format_product_json(product, view),
        // The same envelope, because a new command that invented a second
        // output convention would make `--json` two contracts (#22).
        CommandOutcome::Cache { report } => output::format_cache_json(report, config),
    };

    json_document(meta, data)
}

/// One JSON document and the code the process leaves on.
///
/// Serializing a record we already hold cannot fail in practice, but "cannot
/// fail" is not "need not be answered": under `--json` there is exactly one
/// document on stdout, and a panic here would emit none. It is a bug in this
/// tool rather than anything a caller did, which is what `internal_error` (70)
/// means — the taxonomy's `json_error` was a fifth name for the same thing and
/// no run could reach it, so it is gone.
///
/// The code travels with the string because it used to not: `run` printed this
/// envelope, `ok: false` and all, and then returned `ExitCode::SUCCESS`. A
/// caller branching on the exit code — which is what the whole taxonomy asks it
/// to do — read `0` off a document reporting a failure.
///
/// `pub` so the failing branch is reachable from a test. It is the only way in:
/// no [`ProductDetail`] can be built that `serde_json` refuses.
pub fn json_document(meta: Meta, data: Result<Value, serde_json::Error>) -> (String, u8) {
    match data {
        Ok(data) => (Envelope::success(meta, data).render(), 0),
        Err(e) => (
            Envelope::failure(meta, ErrorKind::Internal, e.to_string()).render(),
            ErrorKind::Internal.exit_code(),
        ),
    }
}

/// The meta block for a run, whether or not its configuration resolved.
///
/// `config` is `None` when the failure happened before the configuration
/// resolved. Without it the envelope's storefront fields are `null`, which is
/// the truth: an unparseable command line has no effective storefront.
fn meta_for(
    config: Option<&AppConfig>,
    provenance: Option<Provenance>,
    emitted_at: SystemTime,
) -> Meta {
    match config {
        Some(config) => Meta::new(config, provenance, emitted_at),
        None => Meta::unconfigured(provenance, emitted_at),
    }
}

/// Report a failed run and produce its exit code.
fn report_failure(config: Option<&AppConfig>, failure: Failure, json: bool) -> ExitCode {
    let kind = classify_error(&failure.error);

    if json {
        let emitted_at = SystemTime::now();
        // The provenance the failure carried, which is `Some` exactly when a
        // page was read before it (#44). A validation failure on a page that
        // loaded used to report `fetched_at: null`, which states of a page that
        // was read that none was.
        let meta = meta_for(config, failure.provenance, emitted_at);
        // `{:#}` rather than `{:?}`: the whole anyhow chain on one line, which
        // is what a JSON string wants, instead of the multi-line debug report
        // the human path prints.
        print!(
            "{}",
            Envelope::failure(meta, kind, format!("{:#}", failure.error)).render()
        );
    } else {
        eprintln!("Error: {:?}", failure.error);
    }

    ExitCode::from(kind.exit_code())
}

/// Report a run Ctrl+C ended, and produce its exit code.
///
/// `--json` promises one document on stdout, always, success or failure — and
/// the interrupt was the one path that promised nothing: exit 130 with **zero
/// bytes written**, measured. An agent that interrupts a fetch got nothing to
/// parse, in the case where "always" matters most.
///
/// Nothing about the interrupt handling itself changes here. By the time this
/// runs the browser has been closed and its profile directory removed (#46);
/// this only writes the document that path never wrote.
fn report_interrupt(config: Option<&AppConfig>, json: bool) -> ExitCode {
    if json {
        let emitted_at = SystemTime::now();
        // No provenance: an interrupt says nothing about whether a page was
        // read, because the command was dropped where it stood.
        let meta = meta_for(config, None, emitted_at);
        print!(
            "{}",
            Envelope::failure(
                meta,
                ErrorKind::Interrupted,
                "Interrupted before the command completed".to_string(),
            )
            .render()
        );
    }

    ExitCode::from(ErrorKind::Interrupted.exit_code())
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
