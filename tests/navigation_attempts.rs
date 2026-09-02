//! #23's last acceptance criterion, and the half of its proposal that the
//! detection commit did not carry: the retry counts are the caller's to set,
//! and a Cloudflare block ends a navigation instead of restarting it.
//!
//! # Why these are counts, and why counts are testable offline
//!
//! Both claims are about *how many times something happens*, which is exactly
//! what a browser test is worst at deciding: a run against the live site cannot
//! be made to fail three times on demand, and a run against a local server
//! cannot be made to look blocked. So the two loops take their work as a
//! closure, the way `wait_for_selectors` has since #11, and these tests count
//! how often the closure was called.
//!
//! The one thing a count cannot catch is the loop being right and the number
//! reaching it being wrong, so the last two tests follow the number the whole
//! way from the flag to the field a `Navigator` is built from.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use iherb_cli::cli::GlobalArgs;
use iherb_cli::config::{AppConfig, DEFAULT_CLOUDFLARE_ATTEMPTS, DEFAULT_NAVIGATION_ATTEMPTS};
use iherb_cli::error::{ErrorKind, IherbError};
use iherb_cli::scraper::navigation::{
    clear_challenge, is_worth_retrying, retry_navigation, ChallengeBudget,
};

/// A challenge budget that costs no wall clock: with no wait there is nothing
/// to poll inside it, so the probe is called once per attempt and no more.
fn instant(attempts: u32) -> ChallengeBudget {
    ChallengeBudget {
        attempts,
        wait: Duration::ZERO,
        poll: Duration::ZERO,
    }
}

async fn nothing() {}

// ---------------------------------------------------------------------------
// A block is not retried
// ---------------------------------------------------------------------------

/// A Cloudflare block ends the navigation sequence at the first attempt (#23).
///
/// The assertion is the call count, not the returned error: an implementation
/// that retried twice more and *then* returned the block would return exactly
/// the same error, having spent the rate limit that is the whole reason not to.
///
/// The elapsed time is asserted too, and it is not decoration. The backoff
/// between attempts is 1s then 2s, so three attempts cannot finish in under
/// three seconds; a run that comes back in milliseconds did not sleep, which is
/// a second, independent witness that it did not try again.
#[tokio::test]
async fn a_cloudflare_block_ends_the_navigation_rather_than_restarting_it() {
    let calls = AtomicU32::new(0);
    let started = Instant::now();

    let result = retry_navigation(3, |_| {
        calls.fetch_add(1, Ordering::Relaxed);
        async { Err(IherbError::CloudflareBlocked(3)) }
    })
    .await;

    let err = result.expect_err("a run that only ever blocked returned a page");
    assert_eq!(
        err.kind(),
        ErrorKind::CloudflareBlocked,
        "the block has to survive as a block, not be relabelled on the way out"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "a blocked page was navigated to again; retrying seconds later from the \
         same address cannot succeed and spends the rate limit a later run wants"
    );
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "no backoff should have been slept, but the sequence took {:?}",
        started.elapsed()
    );
}

/// Every other failure is still retried, up to the configured total (#23).
///
/// The other half of the same decision, and the one that fails if
/// [`is_worth_retrying`] is ever widened into "never retry anything": a
/// navigation that failed because a CDP call did not come back is exactly the
/// failure a second attempt fixes.
///
/// Two attempts rather than three, because the backoff between them is real
/// seconds and one of them is enough to prove the loop continues.
#[tokio::test]
async fn an_ordinary_navigation_failure_is_retried_to_the_configured_total() {
    let calls = AtomicU32::new(0);

    let result = retry_navigation(2, |_| {
        calls.fetch_add(1, Ordering::Relaxed);
        async { Err(IherbError::Navigation("connection reset".to_string())) }
    })
    .await;

    assert!(result.is_err());
    assert_eq!(
        calls.load(Ordering::Relaxed),
        2,
        "a retryable failure must be tried --attempts times in total"
    );

    // And a page that arrives on the second attempt is returned, rather than
    // the loop running to its end and reporting the first failure.
    let calls = AtomicU32::new(0);
    let html = retry_navigation(3, |attempt| {
        calls.fetch_add(1, Ordering::Relaxed);
        async move {
            if attempt == 1 {
                Err(IherbError::Navigation("connection reset".to_string()))
            } else {
                Ok("<html></html>".to_string())
            }
        }
    })
    .await
    .expect("the second attempt succeeded");
    assert_eq!(html, "<html></html>");
    assert_eq!(
        calls.load(Ordering::Relaxed),
        2,
        "a success must end the loop"
    );
}

/// The predicate itself, over every error kind this tool raises (#23).
///
/// A sweep rather than two cases, so that a new variant added to `IherbError`
/// has to be thought about here: the rule is "a block is the exception", and a
/// sweep is what says so about the errors that exist rather than about the two
/// somebody remembered.
#[test]
fn only_a_block_is_not_worth_retrying() {
    let retryable = [
        IherbError::Navigation("connection reset".to_string()),
        IherbError::ParseFailed("no product node".to_string()),
        IherbError::ProductNotFound("99999999".to_string()),
        IherbError::InvalidInput("bad country".to_string()),
    ];
    for e in &retryable {
        assert!(
            is_worth_retrying(e),
            "{:?} is not a block and must still be retried",
            e.kind()
        );
    }
    assert!(!is_worth_retrying(&IherbError::CloudflareBlocked(3)));
}

// ---------------------------------------------------------------------------
// The counts are the caller's
// ---------------------------------------------------------------------------

/// A page that never clears is looked at exactly `--cloudflare-attempts` times.
///
/// Swept over four values rather than asserted at the default, because the
/// number being *configurable* is the criterion and a loop hardcoded to three
/// passes a single-value test at three.
#[tokio::test]
async fn a_challenge_is_looked_at_exactly_the_configured_number_of_times() {
    for attempts in [1, 2, 3, 7] {
        let looks = AtomicU32::new(0);
        let nudges = AtomicU32::new(0);

        let result = clear_challenge(
            instant(attempts),
            || {
                looks.fetch_add(1, Ordering::Relaxed);
                async { true }
            },
            || {
                nudges.fetch_add(1, Ordering::Relaxed);
                nothing()
            },
        )
        .await;

        let err = result.expect_err("a page that never cleared was reported clear");
        assert!(
            matches!(err, IherbError::CloudflareBlocked(n) if n == attempts),
            "the error must report the number of looks actually spent, got {:?}",
            err
        );
        assert_eq!(
            looks.load(Ordering::Relaxed),
            attempts,
            "--cloudflare-attempts {} bought {} looks",
            attempts,
            looks.load(Ordering::Relaxed)
        );
        assert_eq!(
            nudges.load(Ordering::Relaxed),
            attempts - 1,
            "the Turnstile nudge belongs to a wait, and the last attempt does \
             not wait — it reports"
        );
    }
}

/// A page that is not a challenge costs one look and no wait (#23).
///
/// The common case, and the one a greedy loop would make expensive: this runs
/// before every readiness wait on every page the tool fetches.
#[tokio::test]
async fn an_ordinary_page_costs_one_look_and_is_never_nudged() {
    let looks = AtomicU32::new(0);
    let nudges = AtomicU32::new(0);

    clear_challenge(
        instant(3),
        || {
            looks.fetch_add(1, Ordering::Relaxed);
            async { false }
        },
        || {
            nudges.fetch_add(1, Ordering::Relaxed);
            nothing()
        },
    )
    .await
    .expect("a page that is not a challenge is not blocked");

    assert_eq!(looks.load(Ordering::Relaxed), 1);
    assert_eq!(nudges.load(Ordering::Relaxed), 0);
}

/// A challenge that clears ends the wait rather than spending the budget.
#[tokio::test]
async fn a_challenge_that_clears_stops_the_looking() {
    let looks = AtomicU32::new(0);

    // Poll once inside each wait, so the early exit has somewhere to happen.
    let budget = ChallengeBudget {
        attempts: 5,
        wait: Duration::from_millis(1),
        poll: Duration::from_millis(1),
    };

    clear_challenge(
        budget,
        || {
            let n = looks.fetch_add(1, Ordering::Relaxed);
            async move { n == 0 }
        },
        nothing,
    )
    .await
    .expect("the challenge cleared");

    assert_eq!(
        looks.load(Ordering::Relaxed),
        2,
        "one look that saw the challenge, one inside the wait that saw it gone"
    );
}

// ---------------------------------------------------------------------------
// From the flag to the field
// ---------------------------------------------------------------------------

/// `--attempts` and `--cloudflare-attempts` reach the configuration (#23).
///
/// Through `AppConfig::load`, not an `AppConfig` literal: a literal would pass
/// whether or not either flag is wired to anything.
#[test]
fn the_two_flags_carry_their_numbers_into_the_configuration() {
    let mut args = GlobalArgs::none();
    let defaults = AppConfig::load(&args).expect("defaults load");
    assert_eq!(defaults.attempts, DEFAULT_NAVIGATION_ATTEMPTS);
    assert_eq!(defaults.cloudflare_attempts, DEFAULT_CLOUDFLARE_ATTEMPTS);

    args.attempts = Some(7);
    args.cloudflare_attempts = Some(1);
    let stated = AppConfig::load(&args).expect("stated attempts load");
    assert_eq!(
        stated.attempts, 7,
        "--attempts was not carried; the number is still hardcoded somewhere"
    );
    assert_eq!(
        stated.cloudflare_attempts, 1,
        "--cloudflare-attempts was not carried; MAX_CLOUDFLARE_RETRIES lives on"
    );
}

/// Zero is refused rather than clamped, on both flags (#23).
///
/// Clamping to one would silently do the opposite of what the number says. The
/// error is `invalid_input`, which is raised before any browser work — the
/// same class as an unknown country code.
#[test]
fn zero_attempts_is_invalid_input_on_both_flags() {
    for (label, mut args) in [
        ("--attempts", GlobalArgs::none()),
        ("--cloudflare-attempts", GlobalArgs::none()),
    ] {
        if label == "--attempts" {
            args.attempts = Some(0);
        } else {
            args.cloudflare_attempts = Some(0);
        }
        let err = AppConfig::load(&args)
            .err()
            .unwrap_or_else(|| panic!("{} 0 was accepted", label));
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains(label),
            "the error must name the flag to correct, got {}",
            err
        );
    }
}
