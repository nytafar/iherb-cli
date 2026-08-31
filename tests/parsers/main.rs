//! Fixture-based parser tests (#8).
//!
//! Seven real iHerb pages live gzipped under `tests/fixtures/`; see that
//! directory's README for each one's upstream blob, commit and storefront.
//! Every parser that turns bytes into a [`iherb_cli::model`] type is exercised
//! here, all but one of them against a captured page.
//!
//! The one exception is an honest gap, not an oversight.
//! `parse_from_js_globals` runs against JSON transcribed by hand from a
//! captured page's inline `<script>`, because production obtains those globals
//! by evaluating JS in the browser rather than by reading the HTML.
//!
//! There used to be a second exception. The `__NEXT_DATA__` parsers were tested
//! against synthetic JSON because no captured page carried the blob; #34
//! deleted them, and `product_json::next_data_is_absent_from_every_captured_page`
//! is what remains — a guard that says the deletion still holds.
//!
//! Nothing in this target launches a browser, touches the network, or writes
//! outside `tests/fixtures/golden/` and a temp directory the cache tests
//! remove after themselves.
//!
//! # These tests pin what the code does today, not what it should do
//!
//! Bugs #1-#6 are filed and unfixed. Where a parser is known-wrong, the
//! assertion records the wrong answer under a `CHARACTERIZATION, NOT DESIRED`
//! comment naming the issue that will flip it. **Do not change production code
//! to satisfy one of those assertions** — fix the bug in its own issue and flip
//! the assertion there. A plain assertion with no such comment is desired
//! behaviour and a failure is a regression.
//!
//! # Adding a test
//!
//! ```ignore
//! #[test]
//! fn gummies_are_out_of_stock() {
//!     let product = parse_from_json_ld(&OLLY_GUMMIES.json_ld(), "119174", BASE_URL).unwrap();
//!     assert!(!product.in_stock);
//! }
//! ```
//!
//! [`fixture`] hands each parser the shape it wants — `html()`, `doc()`,
//! `json_ld()`, or a JSON side-fixture via `json()`. Adding a page is one line
//! in `fixture.rs`; regenerating the golden Markdown is `UPDATE_GOLDEN=1 cargo
//! test`.

mod fixture;

mod cache_keys;
mod golden;
mod helpers;
mod product_dom;
mod product_json;
mod search;
