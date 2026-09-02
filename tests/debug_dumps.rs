//! Where `--debug` puts the HTML it fetched, and what it calls it.
//!
//! #63. The dump used to go to `/tmp/iherb_<label>.html`, and the label was the
//! whole name. Three consequences, all of them asserted below: two runs against
//! the same target overwrote each other, two concurrent runs raced for one
//! path, and none of it was anywhere near where this tool puts its other files.
//!
//! The assertions are made against the path and the name the production code
//! produces, not against a copy of the format string — `dump_file_name` takes
//! the instant and the pid as arguments precisely so a test can name two of
//! each and compare what comes back.

use std::time::{Duration, SystemTime};

use iherb_cli::config::{dumps_dir, resolve_cache_dir};
use iherb_cli::scraper::helpers::{dump_file_name, dump_path};

/// A fixed instant, so the names below are the test's to reason about.
fn at(offset_secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_756_000_000 + offset_secs)
}

/// #63.3. `/tmp` is not the platform's temp directory and not where this tool
/// puts anything else. The dumps belong under the same resolved cache directory
/// the cache entries use, so they inherit the platform logic that already
/// exists and `cache path` names the directory they are in.
#[test]
fn a_dump_lands_under_the_resolved_cache_directory_and_not_in_tmp() {
    let path = dump_path("product_692");

    assert_eq!(
        dumps_dir(),
        resolve_cache_dir().join("dumps"),
        "dumps must resolve through the cache directory, not a second copy of \
         the platform logic"
    );
    assert_eq!(
        path.parent(),
        Some(dumps_dir().as_path()),
        "a dump must be written into the dumps directory; got {}",
        path.display()
    );
    assert!(
        !path.starts_with("/tmp"),
        "a dump must not go to a hardcoded /tmp path; got {}",
        path.display()
    );
}

/// #63.1, the one most likely to bite. The label was the whole file name, so
/// `iherb_product_692.html` from an hour ago and from just now were the same
/// file. Diffing what iHerb served before and after it changed something — the
/// main reason to keep a dump at all — was impossible without moving files by
/// hand.
#[test]
fn two_runs_against_the_same_target_do_not_overwrite_each_other() {
    let first = dump_file_name("product_692", at(0), 4242);
    let second = dump_file_name("product_692", at(3600), 4242);

    assert_ne!(
        first, second,
        "two runs an hour apart against the same product wrote the same file"
    );
    assert!(
        first.contains("product_692") && second.contains("product_692"),
        "the target must still be readable off the name; got {first} and {second}"
    );
    assert!(
        first < second,
        "the timestamp must sort chronologically, so the newest dump is the \
         last one listed; got {first} then {second}"
    );
}

/// #63.2. Two processes fetching the same id write with no coordination
/// between them, and #10's batch work makes that likelier rather than less.
/// Same target, same instant, different process: different file.
#[test]
fn concurrent_runs_on_the_same_id_do_not_collide() {
    let one = dump_file_name("product_692", at(0), 4242);
    let other = dump_file_name("product_692", at(0), 4243);

    assert_ne!(
        one, other,
        "two processes dumping the same id in the same millisecond raced for \
         one path"
    );
}

/// The timestamp is a timestamp, not just some distinguishing suffix: a reader
/// has to be able to tell when a dump was taken without stat-ing the file.
#[test]
fn the_name_carries_a_readable_utc_timestamp() {
    // 1_756_000_000 seconds after the epoch is 2025-08-24T01:46:40Z.
    let name = dump_file_name("search_magnesium", at(0), 4242);

    assert!(
        name.contains("20250824T014640"),
        "the name must carry the UTC instant the dump was taken; got {name}"
    );
    assert!(
        name.ends_with(".html"),
        "a dump of a page is a .html file; got {name}"
    );
}

/// A search label is whatever the user typed. A `/` in it used to end up in the
/// path, where it either wrote somewhere else or failed — silently either way,
/// because the write is deliberately unchecked.
#[test]
fn a_query_with_a_slash_cannot_escape_the_dumps_directory() {
    let path = dump_path("search_omega_3/6");

    assert_eq!(
        path.parent(),
        Some(dumps_dir().as_path()),
        "a slash in the query moved the dump out of the dumps directory: {}",
        path.display()
    );
}
