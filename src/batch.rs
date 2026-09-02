//! Batch product fetches: many ids over one browser session, streamed as each
//! resolves (#10).
//!
//! `product` used to take exactly one identifier, and every invocation paid
//! the whole fixed cost of a browser launch for that one product — measured
//! at ~6s against iHerb's Norwegian storefront, almost none of it iHerb being
//! slow. This module is what a caller reaches for many ids in one process:
//! one browser, one page per product, and — under `--json` — one compact line
//! per product on stdout as soon as it resolves, rather than one document
//! held back until the whole batch finishes.
//!
//! Batching does **not** reduce the number of fetches — a search results page
//! carries no supplement facts, no serving size, nothing per-nutrient
//! anywhere in its HTML, so the N+1 shape (search, then fetch the shortlist)
//! is iHerb's and it is correct. This module only makes each of the N fetches
//! cheap.
//!
//! # Cache hits never start Chrome
//!
//! Every id is checked against the cache before anything about a browser is
//! decided. A batch that is entirely cache hits returns without launching
//! Chrome at all, and a batch that is partly cache hits pays the launch once,
//! for the misses only.
//!
//! # A bad id does not take the batch with it
//!
//! An id can fail in two places — building the target (not a numeric id or a
//! URL) or fetching it (404, Cloudflare, a parse failure) — and either way the
//! rest of the batch keeps going. The failing id gets its own line, `ok:
//! false`, with the same `error_type` a single-id run would report for the
//! same failure (#59's `product_not_found`, #23's Cloudflare block, and so
//! on). The whole batch only exits non-zero if *every* id failed.
//!
//! # `--concurrency` and the delay
//!
//! `--delay` is politeness between one navigation and the next, and it stays
//! that under concurrency: each worker sleeps it between *its own*
//! consecutive fetches, so `N` workers issue roughly `N` times the request
//! rate of one. There is no shared throttle across workers — dividing the
//! delay by `N` would grant a higher `--concurrency` politeness nobody asked
//! for.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use anyhow::Result;

use crate::browser::session::BrowserSession;
use crate::config::AppConfig;
use crate::error::{classify_error, ErrorKind};
use crate::fetch::{cached, fetch_on, get_or_launch_browser, Provenance};
use crate::model::ProductDetail;
use crate::output::{self, Envelope, Freshness, Meta, ProductView};
use crate::targets::ProductTarget;

/// One id's outcome, resolved and ready to print.
enum Resolved {
    Success {
        /// The id as the record itself carries it, which is the resolved id
        /// rather than whatever form the caller wrote it in (a URL resolves
        /// to the numeric id the record holds).
        product_id: String,
        provenance: Provenance,
        product: Box<ProductDetail>,
    },
    Failure {
        /// The id as the caller wrote it. A [`ProductTarget`] may not exist
        /// to name a resolved id when the identifier itself was the problem.
        requested: String,
        error: anyhow::Error,
        /// `Some` exactly when a page was read before the failure — the same
        /// distinction [`crate::fetch::Failure`] carries for a single fetch
        /// (#44).
        provenance: Option<Provenance>,
    },
}

/// Fold one resolved id into the batch's running verdict.
fn note(resolved: &Resolved, any_success: &mut bool, worst_exit_code: &mut u8) {
    match resolved {
        Resolved::Success { .. } => *any_success = true,
        Resolved::Failure { error, .. } => {
            *worst_exit_code = (*worst_exit_code).max(classify_error(error).exit_code());
        }
    }
}

/// Fetch every id in `ids` over one shared browser session, printing each
/// result to `out` as soon as it resolves.
///
/// Returns the process exit code for the whole batch: `0` if any id
/// succeeded, otherwise the highest exit code among the ids that failed — so
/// a caller who never inspects individual lines still learns that nothing
/// came back, without this issue inventing a new taxonomy entry to say so.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    config: &AppConfig,
    browser_session: &mut Option<BrowserSession>,
    ids: &[String],
    concurrency: usize,
    view: &ProductView,
    json: bool,
    out: &mut impl Write,
) -> Result<u8> {
    let mut misses: Vec<(String, ProductTarget)> = Vec::new();
    let mut any_success = false;
    let mut worst_exit_code = 0u8;

    // Every id is resolved against the cache — or fails outright, for a
    // malformed identifier — before anything about a browser is decided.
    // That is what makes a fully-cached batch never start Chrome: nothing
    // above this point can.
    for id in ids {
        let resolved = match ProductTarget::new(config, id) {
            Ok(target) => match cached(&target, config) {
                Some(fetched) => Some(Resolved::Success {
                    product_id: target.product_id().to_string(),
                    provenance: fetched.provenance,
                    product: Box::new(fetched.data),
                }),
                None => {
                    misses.push((id.clone(), target));
                    None
                }
            },
            Err(error) => Some(Resolved::Failure {
                requested: id.clone(),
                error,
                provenance: None,
            }),
        };

        if let Some(resolved) = resolved {
            note(&resolved, &mut any_success, &mut worst_exit_code);
            print_item(out, config, view, json, &resolved)?;
        }
    }

    if !misses.is_empty() {
        let session = get_or_launch_browser(config, browser_session).await?;
        let cursor = AtomicUsize::new(0);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        // Never more workers than there is work: an idle worker would still
        // poll the cursor once and exit immediately, but it buys nothing.
        let worker_count = concurrency.min(misses.len());
        let workers = futures::future::join_all(
            (0..worker_count).map(|_| worker(&misses, &cursor, config, session, tx.clone())),
        );
        // Only the workers' own clones keep the channel open; once every
        // worker's loop ends, `rx.recv()` sees the channel close and the
        // printer below ends with it.
        drop(tx);

        let printer = async {
            while let Some(resolved) = rx.recv().await {
                note(&resolved, &mut any_success, &mut worst_exit_code);
                print_item(out, config, view, json, &resolved)?;
            }
            Ok::<(), anyhow::Error>(())
        };

        let (_, printed) = tokio::join!(workers, printer);
        printed?;
    }

    Ok(if any_success { 0 } else { worst_exit_code })
}

/// One worker's share of the miss list: pull the next index, wait `--delay`
/// before every fetch after its first, fetch, report.
///
/// Workers share one cursor rather than a pre-split slice so that a fast
/// worker (a quick 404) picks up the next id instead of sitting idle while a
/// slow one is still loading — the same reason a thread pool uses a queue
/// rather than a fixed partition.
async fn worker(
    misses: &[(String, ProductTarget)],
    cursor: &AtomicUsize,
    config: &AppConfig,
    session: &BrowserSession,
    tx: tokio::sync::mpsc::UnboundedSender<Resolved>,
) {
    let mut first = true;
    loop {
        let idx = cursor.fetch_add(1, Ordering::SeqCst);
        let Some((id, target)) = misses.get(idx) else {
            return;
        };

        // Not before this worker's first fetch: politeness is spacing
        // between *this worker's* consecutive navigations, not a warm-up
        // delay before the batch has done anything at all.
        if !first {
            tokio::time::sleep(std::time::Duration::from_millis(config.delay_ms)).await;
        }
        first = false;

        let resolved = match fetch_on(target, config, session).await {
            Ok(fetched) => Resolved::Success {
                product_id: target.product_id().to_string(),
                provenance: fetched.provenance,
                product: Box::new(fetched.data),
            },
            Err(failure) => Resolved::Failure {
                requested: id.clone(),
                provenance: failure.provenance,
                error: failure.error,
            },
        };

        // The receiver only goes away if the printer failed and `run`
        // stopped consuming; nothing left for this worker to do then.
        if tx.send(resolved).is_err() {
            return;
        }
    }
}

/// Print one resolved id and flush immediately.
///
/// The flush is not cosmetic. Rust's stdout is block-buffered once it is not
/// a terminal — which is exactly the case NDJSON exists for, a pipe into
/// `jq` — so "flushed as each product resolves" is a promise that breaks
/// silently without it: the bytes would still all arrive, just batched at
/// the end, indistinguishable from a batch that buffered on purpose.
fn print_item(
    out: &mut impl Write,
    config: &AppConfig,
    view: &ProductView,
    json: bool,
    resolved: &Resolved,
) -> Result<()> {
    let emitted_at = SystemTime::now();

    if json {
        let envelope = match resolved {
            Resolved::Success {
                product_id,
                provenance,
                product,
            } => {
                let meta = Meta::new(config, Some(*provenance), emitted_at);
                match output::format_product_json(product, view) {
                    Ok(data) => Envelope::success(meta, data),
                    Err(e) => Envelope::failure(meta, ErrorKind::Internal, e.to_string()),
                }
                .for_product_id(product_id.clone())
            }
            Resolved::Failure {
                requested,
                error,
                provenance,
            } => {
                let meta = Meta::new(config, *provenance, emitted_at);
                let kind = classify_error(error);
                Envelope::failure(meta, kind, format!("{:#}", error))
                    .for_product_id(requested.clone())
            }
        };
        write!(out, "{}", envelope.render_line())?;
    } else {
        match resolved {
            Resolved::Success {
                provenance,
                product,
                ..
            } => {
                let freshness = Freshness::of(*provenance, emitted_at);
                let doc = output::format_product_document(product, view, freshness, config.debug);
                writeln!(out, "{}", doc)?;
            }
            Resolved::Failure {
                requested, error, ..
            } => {
                writeln!(out, "## Product {}\n\nError: {:#}\n", requested, error)?;
            }
        }
    }

    out.flush()?;
    Ok(())
}
