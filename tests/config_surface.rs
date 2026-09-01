//! The configuration surface #22 was filed about, at the library level.
//!
//! Three things this file is careful about.
//!
//! **The environment is process-wide.** `AppConfig::load` reads
//! `IHERB_BROWSER_PATH`, and `cargo test` runs a binary's tests on parallel
//! threads, so a test that sets it can change what another test resolves. Every
//! test that touches an environment variable takes [`ENV`] first. Nothing
//! outside this file sets that variable, and other test binaries are separate
//! processes.
//!
//! **The cache tests operate on a real directory.** `cache clear` deletes
//! files, so what it deletes and what it refuses to touch has to be checked
//! against a filesystem rather than against a mock — a mock cannot follow a
//! symlink out of the directory, which is the failure worth preventing.
//!
//! **Assertions go through the production path.** `Cache::set` and
//! `Cache::get`, not a hand-written file; `AppConfig::load`, not an `AppConfig`
//! literal. A test that builds its own fixture and asserts on it passes whether
//! or not any code in this crate behaves that way.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

use iherb_cli::cache::{Cache, CacheKey, ClearFilter};
use iherb_cli::cli::{GlobalArgs, SortOrder};
use iherb_cli::config::{parse_duration, AppConfig, CacheMode, DEFAULT_CACHE_TTL};
use iherb_cli::error::{classify_error, ErrorKind};

/// Serializes the tests that mutate the process environment. See the module
/// docs.
static ENV: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> MutexGuard<'static, ()> {
    ENV.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A scratch directory that removes itself.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "iherb-cli-config-{}-{}-{}",
            label,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn path(&self) -> PathBuf {
        self.0.clone()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// `--cache-ttl` and its duration grammar
// ---------------------------------------------------------------------------

/// 30 days was the only TTL there was, hardcoded in `cache.rs` (#22).
#[test]
fn a_duration_is_a_number_and_a_unit() {
    assert_eq!(parse_duration("90s").unwrap(), Duration::from_secs(90));
    assert_eq!(parse_duration("45m").unwrap(), Duration::from_secs(2_700));
    assert_eq!(parse_duration("12h").unwrap(), Duration::from_secs(43_200));
    assert_eq!(
        parse_duration("30d").unwrap(),
        Duration::from_secs(2_592_000)
    );
    assert_eq!(
        parse_duration("2w").unwrap(),
        Duration::from_secs(1_209_600)
    );
    assert_eq!(
        parse_duration(" 7d ").unwrap(),
        Duration::from_secs(604_800)
    );
    assert_eq!(parse_duration("0s").unwrap(), Duration::ZERO);

    // The default this replaces, spelled the way a caller would spell it.
    assert_eq!(parse_duration("30d").unwrap(), DEFAULT_CACHE_TTL);
}

/// **A bare number is rejected.** `--cache-ttl 30` reads as thirty days to one
/// person and thirty seconds to another, and a TTL wrong by a factor of 86,400
/// is a cache that either never hits or never expires. Two keystrokes settle it.
#[test]
fn a_duration_without_a_unit_is_invalid_input() {
    for bad in ["30", "", "d", "-1d", "30 d", "30days", "1.5h", "abc"] {
        let error = parse_duration(bad).unwrap_err_or_panic(bad);
        assert_eq!(
            classify_error(&anyhow::Error::new(error)),
            ErrorKind::InvalidInput,
            "{:?} should be a caller's mistake, not an internal fault",
            bad
        );
    }
}

/// A small helper so the loop above reads as one line per case.
trait UnwrapErrOrPanic<T> {
    fn unwrap_err_or_panic(self, label: &str) -> iherb_cli::IherbError;
}

impl<T: std::fmt::Debug> UnwrapErrOrPanic<T> for Result<T, iherb_cli::IherbError> {
    fn unwrap_err_or_panic(self, label: &str) -> iherb_cli::IherbError {
        match self {
            Ok(value) => panic!("{:?} parsed as {:?} and should not have", label, value),
            Err(e) => e,
        }
    }
}

/// The TTL reaches the cache, so `--cache-ttl` is not a flag that parses and
/// then does nothing.
#[test]
fn the_ttl_decides_whether_an_entry_is_still_a_hit() {
    let dir = TempDir::new("ttl");
    let key = a_key();

    let generous = Cache::new(dir.path(), CacheMode::ReadWrite, Duration::from_secs(3_600));
    generous.set(&key, &"fresh".to_string()).expect("write");
    assert!(
        generous.get::<String>(&key).is_some(),
        "an entry written a moment ago is inside a one-hour TTL"
    );

    // The same file, the same directory, a TTL of nothing.
    let strict = Cache::new(dir.path(), CacheMode::ReadWrite, Duration::ZERO);
    assert!(
        strict.get::<String>(&key).is_none(),
        "a zero TTL makes every entry stale"
    );
}

// ---------------------------------------------------------------------------
// `--no-cache` and `--refresh`
// ---------------------------------------------------------------------------

fn a_key() -> CacheKey {
    CacheKey::Product {
        country: "no".to_string(),
        currency: Some("NOK".to_string()),
        product_id: "12949".to_string(),
    }
}

/// **`--no-cache` writes nothing.** It used to write everything: `Cache::new`
/// took a `no_cache: bool` and set `read_enabled: !no_cache`, leaving writes
/// alone — so a caller asking not to touch the cache got files on disk anyway
/// (#22).
///
/// Asserted against the directory, because the directory is where the claim
/// was false.
#[test]
fn no_cache_touches_the_cache_for_neither_reads_nor_writes() {
    let dir = TempDir::new("no-cache");
    let key = a_key();

    let off = Cache::new(dir.path(), CacheMode::Off, DEFAULT_CACHE_TTL);
    off.set(&key, &"value".to_string()).expect("set reports ok");

    assert!(
        !dir.path().join(key.file_name()).exists(),
        "--no-cache wrote a file"
    );
    assert!(
        std::fs::read_dir(dir.path())
            .expect("the temp dir exists")
            .next()
            .is_none(),
        "--no-cache left something in the cache directory"
    );

    // And it does not read one either, even when a previous run left it there.
    Cache::new(dir.path(), CacheMode::ReadWrite, DEFAULT_CACHE_TTL)
        .set(&key, &"value".to_string())
        .expect("seed");
    assert!(off.get::<String>(&key).is_none(), "--no-cache read a file");
}

/// **`--refresh` skips the read and keeps the answer.** This is what
/// `--no-cache` used to do, and it is a useful thing to be able to ask for —
/// it just is not what "no cache" means.
#[test]
fn refresh_skips_the_read_and_still_writes() {
    let dir = TempDir::new("refresh");
    let key = a_key();

    let refresh = Cache::new(dir.path(), CacheMode::Refresh, DEFAULT_CACHE_TTL);
    refresh.set(&key, &"value".to_string()).expect("write");

    assert!(
        dir.path().join(key.file_name()).exists(),
        "--refresh must write the result it fetched"
    );
    assert!(
        refresh.get::<String>(&key).is_none(),
        "--refresh must not read, not even what it just wrote"
    );

    // A plain run reads it back, which is what makes the write worth anything.
    let plain = Cache::new(dir.path(), CacheMode::ReadWrite, DEFAULT_CACHE_TTL);
    assert_eq!(
        plain.get::<String>(&key).map(|hit| hit.data),
        Some("value".to_string())
    );
}

/// The flags resolve the way the help text says, including the redundant
/// combination.
#[test]
fn the_two_cache_flags_resolve_as_documented() {
    assert_eq!(CacheMode::from_flags(false, false), CacheMode::ReadWrite);
    assert_eq!(CacheMode::from_flags(false, true), CacheMode::Refresh);
    assert_eq!(CacheMode::from_flags(true, false), CacheMode::Off);
    // Redundant rather than contradictory, and the stronger request wins.
    assert_eq!(CacheMode::from_flags(true, true), CacheMode::Off);

    // Through the real configuration loader, so the flags are wired and not
    // merely resolvable.
    let _guard = env_lock();
    let config = AppConfig::load(&GlobalArgs {
        no_cache: true,
        ..GlobalArgs::none()
    })
    .expect("a config with no country is fine");
    assert_eq!(config.cache_mode, CacheMode::Off);

    let config = AppConfig::load(&GlobalArgs {
        refresh: true,
        ..GlobalArgs::none()
    })
    .expect("config");
    assert_eq!(config.cache_mode, CacheMode::Refresh);

    let config = AppConfig::load(&GlobalArgs::none()).expect("config");
    assert_eq!(config.cache_mode, CacheMode::ReadWrite);
    assert_eq!(config.cache_ttl, DEFAULT_CACHE_TTL);
}

// ---------------------------------------------------------------------------
// `--browser-path` and `--config`
// ---------------------------------------------------------------------------

/// **`--browser-path` exists and outranks the environment.**
///
/// It did not exist at all: the executable could be named by
/// `IHERB_BROWSER_PATH` or by the config file, and an agent that cannot set an
/// environment variable on a subprocess had no way to point at a browser — so
/// it fell through to downloading Chrome for Testing (#22).
///
/// All three sources are populated at once and each is distinct, so the test
/// says which one won rather than merely that something was resolved.
#[test]
fn the_browser_path_flag_outranks_the_environment_and_the_file() {
    let _guard = env_lock();
    let dir = TempDir::new("browser-path");
    let from_file = dir.path().join("from-file");
    let from_env = dir.path().join("from-env");
    let from_flag = dir.path().join("from-flag");
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!("[defaults]\nbrowser_path = \"{}\"\n", from_file.display()),
    )
    .expect("write config");

    std::env::set_var("IHERB_BROWSER_PATH", &from_env);

    let with_flag = AppConfig::load(&GlobalArgs {
        browser_path: Some(from_flag.clone()),
        config: Some(config_path.clone()),
        ..GlobalArgs::none()
    })
    .expect("config");
    assert_eq!(with_flag.browser_path.as_deref(), Some(from_flag.as_path()));

    // Drop the flag and the environment takes over; drop that and the file does.
    let without_flag = AppConfig::load(&GlobalArgs {
        config: Some(config_path.clone()),
        ..GlobalArgs::none()
    })
    .expect("config");
    assert_eq!(
        without_flag.browser_path.as_deref(),
        Some(from_env.as_path())
    );

    std::env::remove_var("IHERB_BROWSER_PATH");
    let file_only = AppConfig::load(&GlobalArgs {
        config: Some(config_path),
        ..GlobalArgs::none()
    })
    .expect("config");
    assert_eq!(file_only.browser_path.as_deref(), Some(from_file.as_path()));
}

/// **`--config` reads the file it names, and complains when it cannot.**
///
/// Without it a test or an agent is forced through `~/.config`, which is the
/// user's own and not a place a test may write.
#[test]
fn an_explicit_config_file_is_read_and_its_failures_are_reported() {
    let _guard = env_lock();
    let dir = TempDir::new("config-flag");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[defaults]\ncountry = \"no\"\ncurrency = \"nok\"\ncache_ttl = \"12h\"\ndelay_ms = 50\n",
    )
    .expect("write");

    let config = AppConfig::load(&GlobalArgs {
        config: Some(path.clone()),
        ..GlobalArgs::none()
    })
    .expect("config");
    assert_eq!(config.country, "no");
    assert_eq!(config.currency.as_deref(), Some("NOK"));
    assert_eq!(config.cache_ttl, Duration::from_secs(43_200));
    assert_eq!(config.delay_ms, 50);

    // A flag still outranks the file it was told to read.
    let overridden = AppConfig::load(&GlobalArgs {
        country: Some("de".to_string()),
        config: Some(path),
        ..GlobalArgs::none()
    })
    .expect("config");
    assert_eq!(overridden.country, "de");

    // A path that is not there is a mistake, not a silent fall-through to the
    // defaults — which is how a test passes for the wrong reason.
    let missing = AppConfig::load(&GlobalArgs {
        config: Some(dir.path().join("nowhere.toml")),
        ..GlobalArgs::none()
    })
    .expect_err("a named file that does not exist is an error");
    assert_eq!(missing.kind(), ErrorKind::InvalidInput);

    let broken = dir.path().join("broken.toml");
    std::fs::write(&broken, "this is not = = toml").expect("write");
    let malformed = AppConfig::load(&GlobalArgs {
        config: Some(broken),
        ..GlobalArgs::none()
    })
    .expect_err("a named file that will not parse is an error");
    assert_eq!(malformed.kind(), ErrorKind::InvalidInput);

    // A bad `cache_ttl` in the file fails exactly as a bad `--cache-ttl` does.
    let bad_ttl = dir.path().join("bad-ttl.toml");
    std::fs::write(&bad_ttl, "[defaults]\ncache_ttl = \"30\"\n").expect("write");
    let rejected = AppConfig::load(&GlobalArgs {
        config: Some(bad_ttl),
        ..GlobalArgs::none()
    })
    .expect_err("a bare number is not a duration in the file either");
    assert_eq!(rejected.kind(), ErrorKind::InvalidInput);
}

// ---------------------------------------------------------------------------
// `cache stats` and `cache clear`
// ---------------------------------------------------------------------------

/// Write a cache file with a chosen age.
fn seed(dir: &Path, name: &str, contents: &str, age: Duration) {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write cache file");
    let when = filetime_of(SystemTime::now() - age);
    set_mtime(&path, when);
}

#[cfg(unix)]
fn filetime_of(at: SystemTime) -> libc::timeval {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after the epoch");
    libc::timeval {
        tv_sec: secs.as_secs() as libc::time_t,
        tv_usec: 0,
    }
}

#[cfg(unix)]
fn set_mtime(path: &Path, when: libc::timeval) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).expect("a path with no NUL");
    let times = [when, when];
    // SAFETY: `c` is a valid NUL-terminated path and `times` is a two-element
    // array of `timeval`, which is what `utimes` reads.
    let rc = unsafe { libc::utimes(c.as_ptr(), times.as_ptr()) };
    assert_eq!(rc, 0, "could not set the mtime of {}", path.display());
}

/// `stats` counts exactly what `clear` can remove, and both agree on what a
/// cache entry is.
#[test]
fn stats_counts_the_entries_clear_can_remove() {
    let dir = TempDir::new("stats");
    seed(
        &dir.path(),
        "v4_product_no_NOK_12949.json",
        "0123456789",
        Duration::from_secs(60),
    );
    seed(
        &dir.path(),
        "v4_search_abc123.json",
        "01234",
        Duration::from_secs(120),
    );

    let cache = Cache::new(dir.path(), CacheMode::ReadWrite, DEFAULT_CACHE_TTL);
    let stats = cache.stats().expect("stats");
    assert_eq!(stats.dir, dir.path());
    assert_eq!(stats.entries, 2);
    assert_eq!(stats.bytes, 15);
    assert!(stats.oldest.expect("two entries") < stats.newest.expect("two entries"));

    let cleared = cache.clear(&ClearFilter::default()).expect("clear");
    assert_eq!(cleared.removed.len(), 2);
    assert_eq!(cleared.removed_bytes, 15);
    assert_eq!(cleared.kept, 0);
    assert!(cleared.failed.is_empty());
    assert_eq!(cache.stats().expect("stats").entries, 0);
}

/// A cache directory that has never been written to is an empty cache, not a
/// failure. That is the state a machine that has never run this tool is in.
#[test]
fn a_cache_that_does_not_exist_yet_is_empty_rather_than_broken() {
    let dir = TempDir::new("absent");
    let cache = Cache::new(
        dir.path().join("never-created"),
        CacheMode::ReadWrite,
        DEFAULT_CACHE_TTL,
    );
    let stats = cache.stats().expect("a missing directory is not an error");
    assert_eq!(stats.entries, 0);
    assert_eq!(stats.bytes, 0);
    assert!(stats.oldest.is_none() && stats.newest.is_none());

    let cleared = cache
        .clear(&ClearFilter::default())
        .expect("nothing to clear");
    assert!(cleared.removed.is_empty());
}

/// **`cache clear` deletes user files, so what it will not touch is the
/// assertion that matters.**
///
/// A symlink, a subdirectory, and a file that is not a cache entry. The symlink
/// is the one worth the trouble: `remove_file` on a symlink removes the link
/// and not its target, but a `metadata` call — as opposed to
/// `symlink_metadata` — follows it, and a `clear` that walked into a
/// subdirectory or followed a link would be removing files outside the
/// directory it was told to work in.
#[cfg(unix)]
#[test]
fn clear_never_leaves_the_directory_it_was_given() {
    let dir = TempDir::new("safety");
    let outside = TempDir::new("safety-outside");
    let treasure = outside.path().join("treasure.json");
    std::fs::write(&treasure, "do not delete me").expect("write");

    seed(
        &dir.path(),
        "v4_product_no_NOK_1.json",
        "{}",
        Duration::from_secs(1),
    );
    std::os::unix::fs::symlink(&treasure, dir.path().join("v4_product_no_NOK_link.json"))
        .expect("symlink");
    std::fs::create_dir_all(dir.path().join("nested")).expect("mkdir");
    std::fs::write(dir.path().join("nested/v4_product_no_NOK_2.json"), "{}").expect("write");
    std::fs::write(dir.path().join("notes.txt"), "not a cache entry").expect("write");

    let cache = Cache::new(dir.path(), CacheMode::ReadWrite, DEFAULT_CACHE_TTL);

    // None of the three is an entry, so `stats` does not count them either.
    assert_eq!(cache.stats().expect("stats").entries, 1);

    let cleared = cache
        .clear(&ClearFilter::default())
        .expect("clear the whole cache");
    assert_eq!(
        cleared.removed,
        vec!["v4_product_no_NOK_1.json".to_string()]
    );

    assert!(
        treasure.exists(),
        "clear followed a symlink out of the cache"
    );
    assert_eq!(
        std::fs::read_to_string(&treasure).expect("read"),
        "do not delete me"
    );
    assert!(
        dir.path().join("v4_product_no_NOK_link.json").is_symlink(),
        "the link itself was removed, which is not this command's business either"
    );
    assert!(dir.path().join("nested/v4_product_no_NOK_2.json").exists());
    assert!(dir.path().join("notes.txt").exists());
}

/// `--older-than` keeps what is younger, and `--country` keeps what is not
/// that country — **and says how many entries it could not attribute at all**.
///
/// A search entry is named by a hash of the whole request, so its country is
/// inside the name and cannot be read off it. Reporting "cleared the Norwegian
/// cache" while leaving the Norwegian search results in place is the kind of
/// half-truth a caller acts on, so the count is in the report.
#[test]
fn clear_filters_by_age_and_by_country_and_says_what_it_could_not_attribute() {
    let dir = TempDir::new("filters");
    let day = Duration::from_secs(86_400);
    seed(&dir.path(), "v4_product_no_NOK_old.json", "{}", day * 10);
    seed(
        &dir.path(),
        "v4_product_no_NOK_new.json",
        "{}",
        Duration::from_secs(60),
    );
    seed(
        &dir.path(),
        "v4_product_us_USD_new.json",
        "{}",
        Duration::from_secs(60),
    );
    seed(&dir.path(), "v4_search_abc123.json", "{}", day * 10);

    let cache = Cache::new(dir.path(), CacheMode::ReadWrite, DEFAULT_CACHE_TTL);

    let by_age = cache
        .clear(&ClearFilter {
            older_than: Some(SystemTime::now() - day * 5),
            country: None,
        })
        .expect("clear");
    assert_eq!(
        by_age.removed,
        vec![
            "v4_product_no_NOK_old.json".to_string(),
            "v4_search_abc123.json".to_string()
        ],
        "age says nothing about the storefront, so the search entry goes too"
    );
    assert_eq!(by_age.kept, 2);
    assert_eq!(
        by_age.unattributable, 0,
        "no country filter, nothing to attribute"
    );

    // Put a search entry back, so the country filter has one to be honest about.
    seed(
        &dir.path(),
        "v4_search_def456.json",
        "{}",
        Duration::from_secs(60),
    );

    let by_country = cache
        .clear(&ClearFilter {
            older_than: None,
            country: Some("no".to_string()),
        })
        .expect("clear");
    assert_eq!(
        by_country.removed,
        vec!["v4_product_no_NOK_new.json".to_string()]
    );
    assert_eq!(
        by_country.unattributable, 1,
        "the search entry is unattributable"
    );
    assert_eq!(by_country.kept, 2, "the US entry and the search entry");
    assert!(dir.path().join("v4_product_us_USD_new.json").exists());
    assert!(dir.path().join("v4_search_def456.json").exists());
}

/// A search key really is unattributable, taken from the production key rather
/// than from a name this test invented.
#[test]
fn a_search_entrys_country_is_not_readable_off_its_name() {
    let product = CacheKey::Product {
        country: "no".to_string(),
        currency: Some("NOK".to_string()),
        product_id: "12949".to_string(),
    }
    .file_name();
    assert!(product.contains("_no_"), "{}", product);

    for country in ["no", "us", "de"] {
        let search = CacheKey::Search {
            country: country.to_string(),
            currency: None,
            query: "vitamin c".to_string(),
            sort: SortOrder::Relevance,
            category: None,
        }
        .file_name();
        assert!(
            !search.contains(&format!("_{}_", country)),
            "if the country ever appears in a search file name, `clear --country` \
             can match it and the report must stop saying otherwise: {}",
            search
        );
    }
}

/// **A cache directory that exists and will not open is `cache_unreadable`
/// (12), and that code has a producer.**
///
/// It is not the retired `cache_error` (32). That one claimed an incidental
/// cache failure during a *fetch* could end a run, and none can — a full disk
/// is a log line beside a perfectly good page. This is the case where the whole
/// command is a question about the cache and there is no honest answer.
#[cfg(unix)]
#[test]
fn an_unreadable_cache_directory_is_its_own_code() {
    use std::os::unix::fs::PermissionsExt;

    // Root can read anything, so this proves nothing there.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }

    let parent = TempDir::new("unreadable");
    let dir = parent.path().join("locked");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).expect("chmod");

    let cache = Cache::new(dir.clone(), CacheMode::ReadWrite, DEFAULT_CACHE_TTL);
    let error = cache.stats().expect_err("a directory that will not open");
    assert_eq!(error.kind(), ErrorKind::CacheUnreadable);
    assert_eq!(ErrorKind::CacheUnreadable.exit_code(), 12);
    assert_eq!(
        classify_error(&anyhow::Error::new(error).context("while reading the cache")),
        ErrorKind::CacheUnreadable,
        "the classification has to survive the context the pipeline adds"
    );

    // So the TempDir can clean up after itself.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod back");
}
