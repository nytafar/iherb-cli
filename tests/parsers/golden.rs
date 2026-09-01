//! Golden-output tests for the Markdown formatter.
//!
//! The CLI's whole output is Markdown an agent reads, so a formatting change is
//! a contract change. These render a captured page end to end and diff against
//! a checked-in file; run `UPDATE_GOLDEN=1 cargo test` to rewrite the goldens,
//! then read the diff before committing it.
//!
//! Both bugs the goldens used to characterize have landed, and each grew a
//! golden: `product-104996` gained the `Shipping Weight` line #2 ate and the
//! `## Reviews` histogram #32 could not read. Those diffs were the proof.
//!
//! `product-119174` still has neither section, and that is the page rather than
//! us: the gummies capture carries no `<ugc-review-progress-bar>` at all, so
//! `## Reviews` is genuinely absent and must not be rendered as zeroes.

use std::time::{Duration, SystemTime};

use iherb_cli::cli::Section;
use iherb_cli::fetch::Provenance;
use iherb_cli::model::SearchFetch;
use iherb_cli::output::{
    format_product_detail, format_product_document, format_search_document, format_search_results,
    format_search_shortfall, Freshness, ProductView,
};
use iherb_cli::scraper::product::{enrich_from_html, parse_from_json_ld};
use iherb_cli::scraper::search::parse_search_from_html;

use crate::fixture::{
    assert_golden, BASE_URL, DENTALCIDIN_TUBE, OLLY_GUMMIES, SEARCH_VITAMIN_C, TWO_A_DAY,
};

/// A fixed instant, so a golden can carry a timestamp at all: `2026-09-01
/// 12:34 UTC`.
///
/// Not the clock. `emitted_at` is a parameter everywhere in this tool precisely
/// so a rendering is a function of its inputs (#44), and a golden is the place
/// that matters most.
fn at_noon() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_788_266_040)
}

/// The freshness of a document this run fetched.
fn fresh() -> Freshness {
    Freshness::of(Provenance::Fresh, at_noon())
}

/// The freshness of a document read off a cache file written nine days before
/// the run.
///
/// Nine days rather than nine minutes because the number is the point: this is
/// the case the old `Data from:` bullet lied about, dating a cache hit to the
/// instant it was printed.
fn cached() -> Freshness {
    Freshness::of(
        Provenance::Cached(at_noon() - Duration::from_secs(9 * 24 * 60 * 60)),
        at_noon(),
    )
}

/// The path production takes for a product page: JSON-LD, then DOM enrichment.
fn as_production_would(f: crate::fixture::Fixture) -> iherb_cli::model::ProductDetail {
    // `f.base_url()`, not the `BASE_URL` constant: since #8 the corpus renders
    // Norwegian pages too, and a US literal here would put `www.iherb.com`
    // links under a page that was served from `no.iherb.com`.
    let mut product = parse_from_json_ld(&f.json_ld(), f.product_id(), f.base_url())
        .unwrap_or_else(|| panic!("{}: no JSON-LD", f.slug()));
    enrich_from_html(f.html(), &mut product);
    product
}

#[test]
fn product_renders_every_section() {
    let product = as_production_would(TWO_A_DAY);
    assert_golden(
        "product-104996-full",
        &format_product_document(&product, &ProductView::everything(), fresh(), false),
    );
}

/// The gummies page is missing most of what the formatter can print, so this
/// golden is the shape of a sparse product: no ingredients, no suggested use,
/// no warnings, no review histogram — the page has no widget at all — and an
/// out-of-stock line.
#[test]
fn product_renders_what_a_sparse_page_has() {
    let product = as_production_would(OLLY_GUMMIES);
    assert_golden(
        "product-119174-full",
        &format_product_document(&product, &ProductView::everything(), fresh(), false),
    );
}

/// The first golden of a page that is **not** a dietary supplement, and the
/// first of a page the Norwegian storefront served (#8).
///
/// Dentalcidin is a toothpaste, so there is no `## Nutrition` section to
/// render — not because anything failed to parse, but because a tube of
/// toothpaste has no Supplement Facts panel. Nothing else in the corpus
/// distinguishes "the formatter dropped the section" from "the page has none",
/// and a golden is the only place that distinction is visible end to end.
///
/// It also renders NOK prices and a metric package quantity through a formatter
/// whose every other golden is US and imperial.
///
/// The `Shipping Weight` line in here carries #51's tooltip text, which is
/// filed and not this commit's to fix. It is characterized, not endorsed: when
/// #51 lands, this golden changes and that change is the proof.
#[test]
fn product_renders_a_page_that_is_not_a_supplement() {
    let product = as_production_would(DENTALCIDIN_TUBE);
    assert_golden(
        "product-143499-full",
        // The one full golden rendered off a cache hit, so the footer that says
        // so is pinned somewhere rather than only asserted on.
        &format_product_document(&product, &ProductView::everything(), cached(), false),
    );
}

#[test]
fn a_requested_section_renders_alone() {
    let product = as_production_would(TWO_A_DAY);
    assert_golden(
        "product-104996-nutrition",
        &format_product_document(
            &product,
            &ProductView::for_section(Some(Section::Nutrition)),
            fresh(),
            false,
        ),
    );
    assert_golden(
        "product-104996-overview",
        &format_product_document(
            &product,
            &ProductView::for_section(Some(Section::Overview)),
            fresh(),
            false,
        ),
    );
    // #7's headline case: `--section ingredients` used to render this block and
    // then a stray top-level `- **Data from:**` bullet under it, belonging to no
    // section. The golden is what says it does not any more.
    assert_golden(
        "product-104996-ingredients",
        &format_product_document(
            &product,
            &ProductView::for_section(Some(Section::Ingredients)),
            cached(),
            false,
        ),
    );
}

/// A section the page has no data for prints one honest line rather than an
/// empty heading — **and says when the absence was observed** (#7).
///
/// That clause is not decoration. "No ingredients data available for this
/// product." followed by a timestamp reads as something looked and found
/// nothing just now; on a cache hit nothing looked at all, and the page may
/// have gained the section in the weeks since. An absence is the one claim in
/// this output whose meaning changes with age — a three-week-old price is at
/// least a price that existed — so it carries its own date rather than
/// borrowing the footer's.
#[test]
fn an_absent_section_says_so_and_says_when() {
    let product = as_production_would(OLLY_GUMMIES);

    let fresh_doc = format_product_document(
        &product,
        &ProductView::for_section(Some(Section::Warnings)),
        fresh(),
        false,
    );
    assert!(
        fresh_doc.starts_with(
            "No warnings data available for this product. The page was read during this \
             run and published none."
        ),
        "{:?}",
        fresh_doc
    );

    let cached_doc = format_product_document(
        &product,
        &ProductView::for_section(Some(Section::Reviews)),
        cached(),
        false,
    );
    assert!(
        cached_doc.starts_with(
            "No review data available for this product. That is what the cached record \
             says, and it was read on 2026-08-23 12:34 UTC"
        ),
        "{:?}",
        cached_doc
    );
    assert!(
        cached_doc.contains("the page may have gained one since"),
        "{:?}",
        cached_doc
    );
    // The date is the cache file's, not the run's. An absence dated to the
    // instant the document was printed is the bug this test exists for.
    assert!(
        !cached_doc.contains("2026-09-01 12:34 UTC"),
        "{:?}",
        cached_doc
    );
}

/// **#7's acceptance criterion, as a property of every document this tool
/// emits: no orphan bullets.**
///
/// A top-level list item belongs to whatever heading it sits under. The
/// freshness line used to be one, appended after the formatter returned — so
/// `--section ingredients` produced an `## Other Ingredients` block trailed by
/// a `- **Data from:**` bullet that was not part of it, and a section with no
/// data produced a bullet under no heading at all. Neither is Markdown a caller
/// can parse.
///
/// Swept over every section of every captured page, plus a search, rather than
/// spot-checked: the malformation only showed up for some views, which is
/// exactly why it survived.
#[test]
fn no_document_ever_carries_a_bullet_outside_a_section() {
    let mut documents: Vec<(String, String)> = Vec::new();

    for f in crate::fixture::products() {
        let product = as_production_would(f);
        for freshness in [fresh(), cached()] {
            for view in Section::ALL
                .iter()
                .map(|s| ProductView::for_section(Some(*s)))
                .chain(std::iter::once(ProductView::everything()))
            {
                for health in [false, true] {
                    documents.push((
                        format!("{} {:?}", f.slug(), view.sections()),
                        format_product_document(&product, &view, freshness, health),
                    ));
                }
            }
        }
    }

    let search = parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL).unwrap();
    documents.push((
        "search".to_string(),
        format_search_document(&search, 200, cached()),
    ));

    assert!(documents.len() > 100, "the sweep found nothing to sweep");

    for (label, doc) in &documents {
        let mut seen_heading = false;
        for line in doc.lines() {
            if line.starts_with('#') {
                seen_heading = true;
            }
            if line.starts_with("- ") || line.starts_with("* ") {
                assert!(
                    seen_heading,
                    "{}: a top-level bullet before any heading — it belongs to no \
                     section, which is what #7 is about:\n{}",
                    label, doc
                );
            }
        }

        // The freshness statement is a footer, not a list item, and it is the
        // last thing in the document.
        assert!(
            doc.contains("\n---\n\n*Data "),
            "{}: no freshness footer, set off from the body:\n{}",
            label,
            doc
        );
        let footer = doc
            .rsplit("\n---\n\n")
            .next()
            .expect("rsplit always yields one");
        assert!(
            !footer.contains("- **Data from:**"),
            "{}: the freshness line is a bullet again:\n{}",
            label,
            doc
        );
        assert!(
            footer.starts_with("*Data ") && footer.trim_end().ends_with('*'),
            "{}: the footer is not an emphasis span:\n{:?}",
            label,
            footer
        );
    }
}

#[test]
fn search_results_render() {
    let mut result =
        parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL).unwrap();
    // `cmd_search` truncates to --limit before formatting; five is enough to
    // cover the separator, the discount line and the rating line.
    result.products.truncate(5);
    assert_golden(
        "search-vitamin-c-top5",
        &format_search_document(&result, 5, fresh()),
    );
}

/// A search that came up short of `--limit` says so, and says which kind of
/// short it is. `--limit` counts distinct products (#33), so a short result is
/// ordinary — which is exactly why it has to be stated: a caller counting rows
/// cannot otherwise tell "iHerb had no more" from "we stopped walking" (#6).
#[test]
fn a_search_short_of_the_limit_says_which_kind_of_short() {
    let mut result =
        parse_search_from_html(SEARCH_VITAMIN_C.html(), "vitamin c", BASE_URL).unwrap();
    assert_eq!(result.products.len(), 45);

    // Asked for what we have: nothing to report.
    assert_eq!(format_search_shortfall(&result, 45), None);
    assert_eq!(format_search_shortfall(&result, 10), None);

    // Short because the walk stopped, with 11,952 results behind it.
    result.fetch = SearchFetch {
        pages_fetched: Some(1),
        exhausted: Some(false),
    };
    let note = format_search_shortfall(&result, 200).expect("45 is short of 200");
    assert!(
        note.contains("asked for 200, returning 45 distinct products"),
        "{}",
        note
    );
    assert!(note.contains("more behind these"), "{}", note);

    // Short because there is no more. Same count, opposite advice.
    result.fetch.exhausted = Some(true);
    let note = format_search_shortfall(&result, 200).expect("45 is short of 200");
    assert!(note.contains("iHerb had no more"), "{}", note);

    // A record that does not say is reported as not saying.
    result.fetch = SearchFetch::default();
    let note = format_search_shortfall(&result, 200).unwrap();
    assert!(note.contains("does not say"), "{}", note);
}

/// A block complete except that the offers name no currency.
fn a_product_with_no_currency() -> iherb_cli::model::ProductDetail {
    let no_currency = serde_json::json!({
        "@type": "Product",
        "name": "Acme, Thing, 60 Capsules",
        "brand": { "name": "Acme" },
        "sku": "ACM-1",
        "gtin12": "000000000001",
        "offers": { "price": "9.60", "availability": "https://schema.org/InStock" },
    });
    parse_from_json_ld(&no_currency, "1", BASE_URL).unwrap()
}

/// The `Data quality` line names the expected fields that were not read.
///
/// The record here is degraded by an *absent* currency, which is what offers
/// with no `priceCurrency` now produce: before #5 they produced a hardcoded
/// `"USD"`, so the same record was degraded by a *defaulted* currency instead.
/// `the_degraded_line_names_a_defaulted_field` is the other half — the line has
/// to name both kinds, and only one of them is reachable from a parser.
#[test]
fn the_degraded_line_names_an_absent_expected_field() {
    let product = a_product_with_no_currency();
    let health = product.health();
    assert!(health.degraded);
    assert!(health.fields_absent.contains(&"currency".to_string()));
    // Plenty of *unexpected* fields are absent too — no ingredients, no
    // warnings — and the line must not name those.
    for expected in iherb_cli::model::ProductDetail::EXPECTED_FIELDS {
        if *expected != "currency" {
            assert!(
                !health.fields_absent.contains(&expected.to_string()),
                "{} should have been read",
                expected
            );
        }
    }

    let rendered = format_product_detail(
        &product,
        &ProductView::for_section(Some(Section::Overview)),
        fresh(),
    );
    assert!(
        rendered.contains("- **Data quality:** degraded — no strategy produced currency."),
        "the line must name the field, not print an empty list: {:?}",
        rendered
    );
}

/// The same line, for a field that has a value nobody read.
///
/// A record degraded purely by a *defaulted* expected field used to print an
/// empty list, because the line only ever reported `fields_absent`. Currency
/// was that field until #5 removed the substitution, so the state is now built
/// by hand: no parser produces a defaulted expected field any more, and the
/// branch in `unread_expected_fields` that handles one is still there and still
/// has to work. Building it explicitly is the only way left to say so.
#[test]
fn the_degraded_line_names_a_defaulted_field() {
    let mut product = a_product_with_no_currency();
    product.currency = Some("USD".to_string());
    product
        .extraction
        .reclaim("currency", iherb_cli::model::Source::Defaulted);

    let health = product.health();
    assert!(health.degraded);
    assert!(health.fields_defaulted.contains(&"currency".to_string()));
    assert!(!health.fields_absent.contains(&"currency".to_string()));

    let rendered = format_product_detail(
        &product,
        &ProductView::for_section(Some(Section::Overview)),
        fresh(),
    );
    assert!(
        rendered.contains("- **Data quality:** degraded — no strategy produced currency."),
        "a defaulted field is named exactly as an absent one is: {:?}",
        rendered
    );
}

/// A price whose currency nobody read prints as a number that says so (#5).
///
/// This is the acceptance criterion of #5 at the only place a user meets it.
/// The old formatter took a `&str` and always had one, so `9.60` from the US
/// storefront printed as `CHF 9.60` whenever `--currency CHF` had been passed
/// and detection had failed — a number that is wrong by a factor of the
/// exchange rate, rendered exactly like a number that is right. An unlabelled
/// number costs a reader a second query; a mislabelled one costs them the
/// decision.
#[test]
fn a_price_with_no_currency_says_so_instead_of_borrowing_one() {
    let product = a_product_with_no_currency();
    assert_eq!(product.currency, None);

    let rendered = format_product_detail(
        &product,
        &ProductView::for_section(Some(Section::Overview)),
        fresh(),
    );
    assert!(
        rendered.contains("- **Price:** 9.60 (currency unknown: the page published none)"),
        "{:?}",
        rendered
    );
    // Nothing invents a symbol for it either.
    assert!(!rendered.contains("$9.60"));

    // The same on the search path, where every card on the page is affected at
    // once because iHerb publishes one currency marker for the whole page.
    let unmarked = parse_search_from_html(
        r#"<html><body><div class="product-cell-container">
             <a class="product-link" data-product-id="1" title="Thing" href="/pr/p/1"></a>
             <div class="product-title" content="Thing"></div>
             <meta itemprop="price" content="9.60">
           </div></body></html>"#,
        "q",
        BASE_URL,
    )
    .unwrap();
    let rendered = format_search_results(&unmarked);
    assert!(
        rendered.contains("- **Price:** 9.60 (currency unknown: the page published none)"),
        "{:?}",
        rendered
    );
}

/// A price whose currency *was* read prints with it, unchanged. Without this
/// the test above is satisfied by a formatter that never prints a currency.
#[test]
fn a_price_with_a_currency_still_prints_it() {
    let product = as_production_would(TWO_A_DAY);
    assert_eq!(product.currency.as_deref(), Some("USD"));
    let rendered = format_product_detail(
        &product,
        &ProductView::for_section(Some(Section::Overview)),
        fresh(),
    );
    assert!(rendered.contains("- **Price:** $12.38"), "{:?}", rendered);
    assert!(!rendered.contains("currency unknown"));
}

/// None of the captured pages is degraded on the production path, which is why
/// no golden carries a `Data quality` line. If one starts to, the goldens
/// change and this says why first.
#[test]
fn no_captured_page_is_degraded_on_the_production_path() {
    for f in crate::fixture::products() {
        let product = as_production_would(f);
        assert!(!product.health().degraded, "{}", f.slug());
    }
}

/// The `Data quality` line names only the fields that actually caused the
/// degradation, not every absent field on the page.
///
/// `degraded` is decided by `EXPECTED_FIELDS`, but the line used to print
/// `fields_absent`, which is every absent field there is. The gummies page on
/// the DOM path has several absent fields and only one of them is ever a reason
/// to call the record broken. Naming the innocents sends a reader hunting a
/// selector that is working fine, and is worse than saying nothing.
///
/// The degradation is manufactured, because no capture produces one any more:
/// #2 used to eat `product_code` off every page and this test borrowed that.
/// Relabelling the one spec row costs the DOM path `product_code` and nothing
/// else, which is the single-culprit shape the line is about.
#[test]
fn the_degraded_line_names_only_what_caused_the_degradation() {
    let relabelled = crate::fixture::OLLY_GUMMIES
        .html()
        .replace("Product code:", "Product identifier:");
    let product =
        iherb_cli::scraper::product::parse_from_html(&relabelled, "119174", BASE_URL).unwrap();

    let health = product.health();
    assert!(health.degraded);
    // The innocents: absent, and none of them a reason to call anything broken.
    for innocent in ["ingredients", "suggested_use", "warnings", "original_price"] {
        assert!(
            health.fields_absent.contains(&innocent.to_string()),
            "{} should be absent on this page",
            innocent
        );
    }

    let line = format_product_detail(
        &product,
        &ProductView::for_section(Some(Section::Overview)),
        fresh(),
    );
    assert!(
        line.contains("degraded — no strategy produced product_code."),
        "the line must name the culprit and only the culprit: {:?}",
        line
    );
    for innocent in ["ingredients", "suggested_use", "warnings", "original_price"] {
        assert!(
            !line.contains(innocent),
            "{} is absent but blameless, and must not appear in the degraded line: {:?}",
            innocent,
            line
        );
    }
}

/// A description that came from the `<meta name="description">` fallback is
/// marked as such. It is the full text cut to ~160 characters and it stops
/// mid-phrase, so printing it unmarked shows a reader a sentence that just ends
/// as though that were the product's description.
#[test]
fn a_truncated_description_says_it_is_truncated() {
    let via_dom =
        iherb_cli::scraper::product::parse_from_html(TWO_A_DAY.html(), "104996", BASE_URL).unwrap();

    // The fallback really is what filled it, and it really does stop mid-phrase.
    assert_eq!(
        via_dom.source_of("description"),
        iherb_cli::model::Source::Dom
    );
    let desc = via_dom
        .description
        .clone()
        .expect("the page has a meta description");
    assert!(
        desc.ends_with("California Gold Nutrition® Multivitamin and"),
        "{:?}",
        desc
    );

    let rendered = format_product_detail(
        &via_dom,
        &ProductView::for_section(Some(Section::Description)),
        fresh(),
    );
    assert!(rendered.contains(&desc), "the text itself is unchanged");
    assert!(
        rendered.contains("may stop mid-sentence"),
        "the truncation must be marked: {:?}",
        rendered
    );
    assert!(rendered.contains("#13"), "and point at who fixes it");

    // The JSON-LD description is the full one and carries no such note.
    let via_json_ld = as_production_would(TWO_A_DAY);
    assert_eq!(
        via_json_ld.source_of("description"),
        iherb_cli::model::Source::JsonLd
    );
    let rendered = format_product_detail(
        &via_json_ld,
        &ProductView::for_section(Some(Section::Description)),
        fresh(),
    );
    assert!(
        !rendered.contains("may stop mid-sentence"),
        "{:?}",
        rendered
    );
    assert!(
        via_json_ld.description.unwrap().len() > desc.len(),
        "the structured-data description is the longer one"
    );
}

/// What a reader is told when the page carried a field extraction could not
/// read (#32 round 2).
///
/// **The input is synthetic, and deliberately so.** No captured page has a
/// malformed histogram — the one hydrated widget parses — so there is nothing
/// to characterize here and no golden covers this rendering. Rather than leave
/// it unprotected, the gummies page (which carries no widget of its own) is
/// grafted with a two-bar widget whose bars both claim five stars. That is the
/// same honesty the `next-data-*-synthetic` fixtures were labelled with in #8:
/// hand-written input, named as such, testing a path the captures cannot reach.
///
/// Three things this pins, all of which were wrong the moment `Source::Malformed`
/// existed and before `output.rs` caught up:
///
///  1. The `Data quality` line names the malformed field. It used to print
///     `no strategy produced .` — an empty list and a dangling full stop —
///     because it named only `EXPECTED_FIELDS`, and `review_distribution` is
///     deliberately not one.
///  2. It says the field *was on the page*, not that nothing produced it. Those
///     are different problems with different culprits.
///  3. `format_extraction_health` lists it under `Malformed`, beside the
///     existing `Absent` and `Defaulted` lines, and its `Degraded:` sentence no
///     longer claims a fact that is false here — every field every product page
///     publishes *was* read off this one.
#[test]
fn a_malformed_field_is_rendered_as_unreadable_not_as_missing() {
    // SYNTHETIC: hand-written widget grafted onto a real capture. See above.
    const BROKEN_WIDGET: &str = r#"<ugc-review-progress-bar>
          <button class="item"><span>5 stars</span>
            <div class="percent-wrap"><span class="block" style="width: 84%;"></span></div></button>
          <button class="item"><span>5 stars</span>
            <div class="percent-wrap"><span class="block" style="width: 1%;"></span></div></button>
        </ugc-review-progress-bar><ul id="product-specs-list""#;
    let grafted = OLLY_GUMMIES
        .html()
        .replace(r#"<ul id="product-specs-list""#, BROKEN_WIDGET);
    assert_ne!(grafted, OLLY_GUMMIES.html(), "the graft must have taken");

    let mut product = parse_from_json_ld(&OLLY_GUMMIES.json_ld(), "119174", BASE_URL).unwrap();
    enrich_from_html(&grafted, &mut product);

    let overview = format_product_detail(
        &product,
        &ProductView::for_section(Some(Section::Overview)),
        fresh(),
    );
    assert!(
        overview.contains("degraded — review_distribution was on the page and could not be read."),
        "the line must name the field and say what went wrong: {:?}",
        overview
    );
    assert!(
        !overview.contains("no strategy produced ."),
        "the empty-list, dangling-period sentence must not come back: {:?}",
        overview
    );

    let health = iherb_cli::output::format_extraction_health(&product.health());
    assert!(
        health.contains("- **Malformed (on the page, unreadable):** review_distribution"),
        "{:?}",
        health
    );
    assert!(
        health.contains("a field the page carried could not be read"),
        "the Degraded sentence must cover this cause, not just the other one: {:?}",
        health
    );

    // The same page untouched says none of it: no widget, no complaint, and the
    // `Data quality` line stays absent entirely.
    let intact = as_production_would(OLLY_GUMMIES);
    assert!(!intact.health().degraded);
    assert!(!format_product_detail(
        &intact,
        &ProductView::for_section(Some(Section::Overview)),
        fresh()
    )
    .contains("Data quality"));
}
