//! Cloudflare challenge classification (#23).
//!
//! # The honest-testing problem, stated before the tests
//!
//! **This programme has never received a live Cloudflare challenge.** Twenty-
//! eight searches and twelve captures against two storefronts, and not one
//! interstitial. Clearance is therefore *unmeasured*, not confirmed, and no
//! test in this file can be written from a captured challenge page because none
//! exists to capture from.
//!
//! That shapes what is asserted here, in two directions that are not equally
//! strong and are not presented as if they were.
//!
//! | direction | evidence | strength |
//! |---|---|---|
//! | a challenge is classified as one | one **synthesized** page, reconstructed from Cloudflare's published markup | it proves the classifier's rules, not that iHerb's challenge matches them |
//! | an ordinary page is **not** | all twenty-three pages this repository actually received | as strong as the corpus |
//!
//! The second direction is the one this change can break, and it is the one
//! backed by real pages. That is deliberate. A test that fires only when a
//! challenge arrives is a test that has never run; a test over pages that do
//! arrive fails the moment the classifier gets greedy, which is the mistake
//! #23 explicitly warned against porting from the fork.
//!
//! # Why the signals, and not the HTML
//!
//! [`is_challenge_page`] judges a [`ChallengeSignals`], not markup. Two of its
//! four fields cannot be read off static HTML at all — `body.innerText` is what
//! a renderer computed, and `content_present` is a live query — so the browser
//! is where they come from and a fixture cannot supply them directly.
//!
//! [`signals_from_html`] below is how a fixture is turned into signals for a
//! test, and it is **deliberately more permissive than the browser**: its body
//! text is every text node under `<body>` with script and style contents
//! removed, which is a superset of `innerText` because it also includes text a
//! renderer would have hidden. A page that is not classified as a challenge on
//! the superset cannot be classified as one on `innerText`. The negative sweeps
//! are therefore stronger than production, not weaker.
//!
//! The element check is not an approximation: it runs
//! [`CHALLENGE_ELEMENT_SELECTORS`] — the same constant the browser probe
//! interpolates into its script — over the parsed document.

use iherb_cli::scraper::navigation::{
    challenge_probe_script, is_challenge_page, ChallengeSignals, CHALLENGE_ELEMENT_SELECTORS,
};
use scraper::{Html, Selector};

use crate::fixture::{self, CHALLENGE_SYNTHETIC, NOT_FOUND_US, SEARCH_VITAMIN_D3_PRICE_ASC};

/// The signals a browser would report for this page, with `content_present`
/// stated by the caller because no static document can answer it.
///
/// See the module docs for why the body text is a superset of `innerText`.
fn signals_from_html(html: &str, content_present: Option<bool>) -> ChallengeSignals {
    let doc = Html::parse_document(html);

    let title = doc
        .select(&Selector::parse("title").unwrap())
        .next()
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default();

    let ignore = Selector::parse("script, style, noscript").unwrap();
    let ignored: Vec<_> = doc.select(&ignore).map(|el| el.id()).collect();
    let body_text = doc
        .select(&Selector::parse("body").unwrap())
        .next()
        .map(|body| {
            body.descendants()
                .filter(|node| {
                    !node
                        .ancestors()
                        .chain(std::iter::once(*node))
                        .any(|a| ignored.contains(&a.id()))
                })
                .filter_map(|node| node.value().as_text().map(|t| t.to_string()))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();

    let challenge_element = CHALLENGE_ELEMENT_SELECTORS.iter().any(|s| {
        let selector = Selector::parse(s).unwrap_or_else(|e| panic!("{}: {:?}", s, e));
        doc.select(&selector).next().is_some()
    });

    ChallengeSignals {
        title,
        body_text,
        challenge_element,
        content_present,
    }
}

// ---------------------------------------------------------------------------
// The direction backed by real pages
// ---------------------------------------------------------------------------

/// No page iHerb has actually served here is classified as a challenge (#23).
///
/// Every capture in the repository — twenty product and listing pages, the two
/// not-found captures — under the **hardest** setting: `content_present:
/// Some(false)`, which arms the weak tier. Nothing is relying on a readiness
/// selector having matched, so the only things standing between these pages and
/// a false `cloudflare_blocked` are the strong signals being specific and the
/// weak text markers being absent from rendered text.
///
/// This is the assertion that fails if the detection is ever widened the way
/// #23 warned against.
#[test]
fn no_page_iherb_served_is_classified_as_a_challenge() {
    let mut checked = 0;
    for f in fixture::pages_iherb_served() {
        let signals = signals_from_html(f.html(), Some(false));
        assert!(
            !is_challenge_page(&signals),
            "{}: an ordinary iHerb page was classified as a Cloudflare challenge \
             (title={:?}, element={})",
            f.slug(),
            signals.title,
            signals.challenge_element,
        );
        checked += 1;
    }
    assert_eq!(
        checked, 23,
        "the sweep must cover every captured page; it covered {}",
        checked
    );
}

/// The fork's marker set would report ordinary pages as blocked (#23).
///
/// Not a test of this repository's code. It pins the *measurement* that decided
/// the design, so that "do not port the fork's approach verbatim" stays an
/// argument with evidence rather than a remembered warning. The fork matches
/// `"Cloudflare"`, `"cf-turnstile"` and `"challenge-platform"` against
/// `documentElement.innerHTML`; iHerb is behind Cloudflare on every page it
/// serves, so its ordinary pages carry the bootstrap in a script tag.
///
/// If this ever fails because the strings left the captured pages, the fork's
/// approach is still wrong for the reason above — but the concrete
/// counterexample would need rewriting rather than deleting.
#[test]
fn the_forks_markers_hit_pages_that_are_not_challenges() {
    const FORK_MARKERS: &[&str] = &["Cloudflare", "cf-turnstile", "challenge-platform"];

    for f in [SEARCH_VITAMIN_D3_PRICE_ASC, NOT_FOUND_US] {
        let html = f.html();
        let hit = FORK_MARKERS.iter().find(|m| html.contains(*m));
        assert!(
            hit.is_some(),
            "{}: expected a fork marker in the markup of an ordinary page",
            f.slug()
        );

        // And this repository's classifier does not repeat the mistake.
        assert!(
            !is_challenge_page(&signals_from_html(html, Some(false))),
            "{}: carries {:?} and must still not be a challenge",
            f.slug(),
            hit
        );
    }
}

// ---------------------------------------------------------------------------
// The direction backed by a synthesized page
// ---------------------------------------------------------------------------

/// A Cloudflare challenge is classified as one (#23).
///
/// Against `cloudflare-managed-challenge-synthetic`, which is **reconstructed
/// from Cloudflare's published markup and not captured** — see that fixture's
/// own documentation for what that does and does not prove. What is asserted is
/// the classifier's rule, and it is asserted twice: once on the whole page, and
/// once with the title removed, because a Turnstile challenge that sets no
/// matching title is exactly the case the old title-only check missed.
#[test]
fn a_challenge_page_is_classified_as_blocked() {
    let html = CHALLENGE_SYNTHETIC.html();

    let signals = signals_from_html(html, Some(false));
    assert!(
        signals.challenge_element,
        "the fixture has no challenge element"
    );
    assert!(is_challenge_page(&signals));

    // Title-blind: the structural signal has to carry it alone.
    let untitled = ChallengeSignals {
        title: String::new(),
        ..signals_from_html(html, Some(false))
    };
    assert!(
        is_challenge_page(&untitled),
        "a challenge with no recognisable title was let through"
    );

    // Element-blind: the title has to carry it alone, which is what a page
    // whose markup Cloudflare has since changed comes down to.
    let elementless = ChallengeSignals {
        challenge_element: false,
        body_text: String::new(),
        ..signals_from_html(html, Some(false))
    };
    assert!(
        is_challenge_page(&elementless),
        "a challenge titled {:?} was let through",
        elementless.title
    );
}

/// A localized interstitial is a challenge, which is the whole of #23 (#23).
///
/// The title markers are carried from the `caozhuozi` fork on the fork's word;
/// nothing here has seen either arrive. The test says what it can say: given
/// that title, the classification is `blocked` and not `product_not_found`.
#[test]
fn a_localized_title_is_a_challenge() {
    for title in [
        "请稍候",
        "正在进行安全验证",
        "Just a moment...",
        "ATTENTION REQUIRED!",
    ] {
        let signals = ChallengeSignals {
            title: title.to_string(),
            content_present: Some(false),
            ..ChallengeSignals::default()
        };
        assert!(is_challenge_page(&signals), "{:?} was let through", title);
    }
}

// ---------------------------------------------------------------------------
// The weak tier's fence
// ---------------------------------------------------------------------------

/// Cloudflare's visible copy blocks only when the page's own content is absent.
///
/// The three cases are the whole of the weak tier, and the second and third are
/// why it is a tier rather than another marker in the list. A page that mentions
/// Cloudflare *and* rendered its product data is a product page; a page whose
/// shape nothing here knows (`content_present: None`) is not evidence of
/// anything and must not block a run.
#[test]
fn the_weak_tier_needs_the_absence_of_content() {
    let mention = "This site is protected by Cloudflare. Verifying you are human.";

    let no_content = ChallengeSignals {
        body_text: mention.to_string(),
        content_present: Some(false),
        ..ChallengeSignals::default()
    };
    assert!(is_challenge_page(&no_content), "weak signal + no content");

    let with_content = ChallengeSignals {
        content_present: Some(true),
        ..no_content.clone()
    };
    assert!(
        !is_challenge_page(&with_content),
        "a page that rendered its own data is not a challenge for mentioning Cloudflare"
    );

    let unknown = ChallengeSignals {
        content_present: None,
        ..no_content.clone()
    };
    assert!(
        !is_challenge_page(&unknown),
        "a target with no readiness selector cannot report content as absent"
    );
}

/// The word in the markup is not the word on the page (#23).
///
/// The fork's failure in one assertion: the same string, once inside a script
/// tag and once in rendered text, on a page with no content selector showing.
/// `innerText` is what makes those two different inputs, and being different is
/// the entire reason this detector reads it instead of `innerHTML`.
#[test]
fn a_cloudflare_string_in_a_script_is_not_a_challenge() {
    let markup_only = "<html><head><title>Nordic Naturals Ultimate Omega</title></head>\
        <body><h1>Ultimate Omega</h1>\
        <script>var s='/cdn-cgi/challenge-platform/scripts/jsd/main.js';</script>\
        </body></html>";
    assert!(!is_challenge_page(&signals_from_html(
        markup_only,
        Some(false)
    )));

    let rendered = "<html><head><title>Nordic Naturals Ultimate Omega</title></head>\
        <body><h1>Verifying you are human</h1><p>Performance &amp; security by Cloudflare</p>\
        </body></html>";
    assert!(is_challenge_page(&signals_from_html(rendered, Some(false))));
}

// ---------------------------------------------------------------------------
// The contract between the browser script and the struct it fills
// ---------------------------------------------------------------------------

/// The probe script asks for every signal, and its answer deserializes (#23).
///
/// The failure this catches is silent and total. `probe_challenge` falls back
/// to `ChallengeSignals::default()` when deserialization fails, and the default
/// is "not a challenge" — so renaming a field on either side turns the detector
/// off for every page, forever, with nothing in the log to say so. Before #23
/// the probe read one string and there was nothing to get out of step.
///
/// The JSON below is what Node produced from this exact script against a stub
/// document, which is as close to the browser as an offline suite gets.
#[test]
fn the_probe_script_and_the_signals_struct_agree() {
    let product = challenge_probe_script(&["h1#name", "#product-overview"]);
    for field in ["title", "body_text", "challenge_element", "content_present"] {
        assert!(product.contains(field), "the script never reads {}", field);
    }
    for selector in CHALLENGE_ELEMENT_SELECTORS {
        // As it must appear inside a JavaScript string literal: any `"` in a
        // selector is escaped, which is the reason the array is built through
        // `serde_json` and not by joining with quotes. An unescaped quote ends
        // the string early and the script throws, and a script that throws is
        // "not a challenge" for every page.
        let in_script = selector.replace('"', "\\\"");
        assert!(
            product.contains(&in_script),
            "the script does not look for {} (expected {})",
            selector,
            in_script
        );
    }
    assert!(
        product.contains("h1#name"),
        "the content selectors are not sent"
    );
    assert!(
        product.contains("body.innerText") || product.contains("body && body.innerText"),
        "the script must read innerText and never innerHTML"
    );
    assert!(
        !product.contains("innerHTML"),
        "reading innerHTML is the fork's mistake (#23)"
    );

    // A target with no readiness selectors reports content as unknown, not
    // absent — which is what keeps the weak tier disarmed there.
    let unknown = challenge_probe_script(&[]);
    assert!(
        unknown.contains("content_present: null"),
        "a target with no selectors must send null, not false"
    );

    let from_browser = r#"{"title":"Just a moment...","body_text":"Verifying you are human",
        "challenge_element":true,"content_present":false}"#;
    let signals: ChallengeSignals = serde_json::from_str(from_browser).expect("script output");
    assert_eq!(signals.title, "Just a moment...");
    assert!(signals.challenge_element);
    assert_eq!(signals.content_present, Some(false));
    assert!(is_challenge_page(&signals));

    let unknown_content = r#"{"title":"","body_text":"","challenge_element":false,
        "content_present":null}"#;
    let signals: ChallengeSignals = serde_json::from_str(unknown_content).expect("script output");
    assert_eq!(signals.content_present, None);
}
