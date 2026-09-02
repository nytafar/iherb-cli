# Fixtures

Real iHerb pages, used by the parser tests in `tests/parsers/`. Each
`*.html.gz` is the page exactly as served, gzipped and renamed to an ASCII
slug — nothing is trimmed, because a trim would quietly discard the markup some
later bug turns out to be about.

There are **two corpora here, and they are not interchangeable.**

| | legacy regression corpus | current corpus |
|---|---|---|
| what | seven pages inherited from an upstream fork | thirteen pages this repository captured |
| storefront | US, all seven | Norway, all thirteen |
| currency | USD | NOK |
| siteVersion | 1.0.19891 – 1.0.20071 | 1.0.22698 |
| captured | 2026-05-29 – 2026-06-05 (commit dates; capture dates unrecorded) | 2026-08-31 and 2026-09-01 |
| forms | veggie caps, gummies, softgels, one 250 g powder | capsules, softgels, tablets, micro tablets, delayed-release veggie caps, powder by the gram, liquid by the millilitre |
| products in common | **none** | |

Twenty files, 55.6 MB of HTML, 7.7 MB gzipped.

**New parser work should be designed against the current corpus.** It is the
site as it is served today, in the storefront this tool is actually pointed at,
for the products it is actually used for.

**The legacy corpus is kept and must not be deleted.** Characterization tests
are pinned to those pages, and deleting them erases what those tests encode.
They are also the only record here of the older markup, which is the only thing
that can tell a parser change from a site change.

### Why the split is worth writing down

The legacy corpus was a monoculture, and a monoculture makes assertions that
cannot fail. Two bugs are already on record:

- **#51** (`shipping_weight` returns its whole info tooltip) was invisible to
  all seven pages and appeared the instant a current page was captured.
- **#52** (a bare `$` read as USD) and four currency sweeps that "could never
  have failed" were both consequences of every page being priced in one
  currency.

#8's captures broke a third on their first run, in
`product_dom::supplement_facts_parse_on_every_product_page`: it required *every*
product page to carry a Supplement Facts panel and to state servings per
container. Both held for eight pages that were all swallowable US supplements;
five of the twelve new pages contradict them. What replaced that sweep is in
`tests/parsers/product_dom.rs`, and the parser gap it uncovered on the way —
`Serving Per Container` in the singular, read past — is **#54**.

## Where the seven upstream pages came from

Upstream: **`https://github.com/caozhuozi/iherb-cli`**, committed under
`fixtures/` by `caozhuozi <543481992@qq.com>` across three commits. That fork's
`master` is mutable and its remote is no longer configured here, so every file
below is pinned to an **immutable git blob id** rather than to a branch.

| upstream commit | date | files it introduced |
|---|---|---|
| `14599ed6ed77479f6cac3740ddb4ff5ad5166fb6` | 2026-05-29 | 104996, 108255, 59561, search |
| `f00e617a9711b4211bf5fbdde459f912da70308b` | 2026-05-31 | category |
| `4ba52741290ae2dfad1d4e91fc11b6acb3a3638c` | 2026-06-05 | 119174, 12949 |

**Capture dates are not recorded upstream.** The commit date is an upper bound;
each page's own `window.IHR_DL.siteVersion` is the tighter fingerprint, and it
groups the files consistently with the commits (19891 < 19940 < 20071). Every
page is the **US storefront** — `countryCode` is `US`, `<html lang="en">`, and
`detect_currency_from_html` returns `USD` on all seven. All are the
server-rendered page *after* client hydration: the review widgets, custom
elements and `class="hydrated"` markers are present in the HTML.

### Per file

| slug (here) | blob id | commit | siteVersion | upstream path |
|---|---|---|---|---|
| `product-104996-cgn-two-a-day` | `0d877db8970ed2f7ff036e6d593d9dfb88b6853d` | `14599ed` | 1.0.19891 | `fixtures/iherb-product-104996-california-gold-nutrition-multivitamin-and-mineral-with-methyl-b12-vitamin-c-l-methylfolate-and-quercetin-two-a-day-60-veggie-capsules.htm` (note: `.htm`) |
| `product-108255-cgn-b-complex` | `01c6f5c4df612039e060feb84f6e36337da53dda` | `14599ed` | 1.0.19891 | `fixtures/iherb-product-108255-california-gold-nutrition-high-potency-vitamin-b-complex-with-methyl-folate-methyl-b12-90-veggie-capsules.html` |
| `product-119174-olly-gummies` | `fecb91b3daaf576d8d6cea058c910f5a20567f79` | `4ba5274` | 1.0.20071 | `fixtures/iherb-product-119174-OLLY, Goodbye Stress®, Berry Verbena, 42 Gummies.html` |
| `product-12949-nordic-ultimate-omega` | `d4e7278c534b1806626927c68b0c49a3ad4fbc6d` | `4ba5274` | 1.0.20071 | `fixtures/iherb-product-12949-Nordic Naturals, Ultimate Omega®, Great Lemon, 180 Soft Gels (640 mg per Soft Gel).html` |
| `product-59561-cgn-gold-c-powder` | `5b5b6214221cba3b85e1da5efc64a78b1b5f6785` | `14599ed` | 1.0.19891 | `fixtures/iherb-product-59561-california-gold-nutrition-gold-c-powder-usp-grade-vitamin-c-1-000-mg-8-81-oz-250-g.html` |
| `search-vitamin-c` | `d9bbc67c97273dd7916c142ec916e1f18729ec29` | `14599ed` | 1.0.19940 | `fixtures/iherb-search-vitamin_c.html` |
| `category-supplements` | `da8c055c2030c75f2fa76e4ea779a1ddf4417a4e` | `f00e617` | 1.0.19940 | `fixtures/Supplements _ Natural Dietary Supplements _ iHerb.html` |

Upstream names contain spaces, commas and `®`; the slugs are the ASCII renames.

## The current corpus

Thirteen pages, all from `no.iherb.com`, all priced in NOK, all at siteVersion
`1.0.22698`. None came from the fork; this repository captured every one of
them, with the command each row records.

### The first of them, and what it proves on its own

`product-12949-nordic-ultimate-omega-nok` (#5) came first and is documented
separately because its provenance argument is different from the rest: it is the
*same product* as a legacy page, which is what lets it isolate the storefront.

| | |
|---|---|
| slug | `product-12949-nordic-ultimate-omega-nok` |
| blob id | `f08f4d82443f304ac4dc121306547e32fd3e439f` |
| URL | `https://no.iherb.com/pr/item/12949` (canonical: `https://no.iherb.com/pr/nordic-naturals-ultimate-omega-great-lemon-180-soft-gels-640-mg-per-soft-gel/12949`) |
| captured | 2026-08-31 |
| storefront | `NO` — `COUNTRY_CODE = "NO"` |
| currency | `NOK` — `CURRENCY_CODE = "NOK"`, `"priceCurrency":"NOK"`, price 880.63 |
| siteVersion | 1.0.22698 |
| capture method | `iherb-cli product 12949 --country no --currency NOK --no-cache --debug`, then `gzip -9 -n` of the dump that `--debug` writes under `$(iherb-cli cache path)/dumps` (it was `/tmp/iherb_product_12949.html` at capture time; #63 moved it) |

Same shape as the other seven: the whole page as served, after hydration,
nothing trimmed. It is the **same product** as
`product-12949-nordic-ultimate-omega`, which is the point — the two differ only
in the storefront that priced them, so a test can hold the product constant and
watch the currency and the price change.

### What this file does and does not prove

It proves the parsers read a non-USD page correctly. It does **not**, by itself,
prove that `--currency` caused it: the machine that captured it is in Norway,
and iHerb geolocates by IP, so `no.iherb.com` serves NOK to this address with or
without the flag. A committed blob cannot record what produced it.

What the cookie is proven by is the experiment recorded in
`tests/storefront_cookie.rs`, run the same day from the same address against the
same product:

| request | `COUNTRY_CODE` | `CURRENCY_CODE` | price |
|---|---|---|---|
| no `--currency` at all | `NO` | `NOK` | NOK 880.63 |
| `--country us --currency USD` | `US` | `USD` | $64.56 |
| `--country de --currency EUR` | `DE` | `EUR` | €76.57 |

The first row is the control: it is what this address gets when nothing is
asked for, which is why the other two are evidence rather than coincidence.

### The twelve that followed (#8)

Captured **2026-09-01**, all thirteen at siteVersion `1.0.22698`, all
`COUNTRY_CODE = "NO"` and `CURRENCY_CODE = "NOK"`, all the whole page as served
after hydration with nothing trimmed.

These are not a sample of iHerb. They are the products the tool's user actually
buys, resolved from a supplement registry that records a brand and a form and
**never an iHerb id** — the join is in `registry-map.md`, along with the four
rows it could not settle. #43 `resolve` is what will eventually automate it.

They were chosen to span **forms** rather than the alphabet, because #15 is the
programme's gate issue and decides the structured-quantity and container model
that #16 and #17 build on. The registry's real derived units are kr/cap, kr/g,
kr/softgel and kr/tablet, and it contains a 90 ml liquid and a 250 g powder;
designing that model against the gummies and veggie capsules of the legacy
corpus is how it gets rewritten later.

| slug | product id | blob id | price on the page | registry note |
|---|---|---|---|---|
| `product-118148-swanson-fiberaid-arabinogalactan-nok` | `118148` | `eeeddcc009b6f7beef2db9ee7c7973efeda91395` | NOK 277.99 | `arabinogalactan` |
| `product-143499-biocidin-dentalcidin-toothpaste-nok` | `143499` | `3479f1f19d402b1efdedfef78f9ba1934d37b498` | NOK 300.55 | `dentalcidin` |
| `product-35060-arg-butyren-tributyrin-nok` | `35060` | `2923e8d457cc29cfd244a1b8dcc62d3b38a5dd01` | NOK 285.78 | `tributyrin` |
| `product-78419-kal-lithium-orotate-nok` | `78419` | `e95e27ec8f5eb043564335a8cbe071f88865894c` | NOK 98.19 | `lithium-orotate` |
| `product-117699-swanson-supreme-c-complex-nok` | `117699` | `cacc52f53d8e6c98b085e4de962a764284dcee64` | NOK 305.03 | `vitamin-c-complex` |
| `product-124094-nutricost-k2-mk7-nok` | `124094` | `c3517cf8b6db196e02d68e30c757051bdd201b57` | NOK 198.90 | `vitamin-k2` |
| `product-16790-enzymedica-digest-gold-nok` | `16790` | `658e2cc2d66508c643cb2849aa6329e9bfc3273c` | NOK 1079.10 | `digestive-enzymes` |
| `product-105890-bodybio-calcium-magnesium-butyrate-nok` | `105890` | `6ade1a27d54a4aa98d0cf3c2d3d3090fbb831a5e` | NOK 650.08 | `calcium-magnesium-butyrate` |
| `product-12081-country-life-coenzyme-b-complex-nok` | `12081` | `b1c3ca56f7793ca28310b6e418362ee4c8d1640d` | NOK 520.19 | `b-complex` |
| `product-75722-dynamic-health-tart-cherry-nok` | `75722` | `512a6e16988b2f01d25951028db31992ef793287` | NOK 408.53 | `tart-cherry-concentrate` |
| `product-4-doctors-best-r-lipoic-acid-nok` | `4` | `0a26ad3b26db4115b2e14967c454c358c2463f32` | NOK 259.56 | `r-lipoic-acid` |
| `product-132364-humanx-gasseri-reuteri-nok` | `132364` | `4296de3c4570eaa1d90707cedf032f9f42d74f00` | NOK 303.48 | `lactobacillus-gasseri-reuteri` |

Every one was captured the same way, which is the way `#5` established:

```sh
iherb-cli product <id> --country no --currency NOK --no-cache --debug
# The dump lands in $(iherb-cli cache path)/dumps, named
# iherb_product_<id>_<timestamp>_<pid>.html since #63; it was
# /tmp/iherb_product_<id>.html when these were captured.
gzip -9 -n <the dump> > product-<id>-<slug>-nok.html.gz
```

Serially, one at a time, at the default 2000 ms delay. **iHerb never presented a
Cloudflare challenge** across the 28 resolution searches and 12 captures this
took — consistent with every prior wave, and still an observation rather than a
guarantee. Nothing in this repository has yet measured what happens when one
appears.

The blob ids are the identity of the *uncompressed* bytes and are checked the
same way as the upstream ones, below.

### Verifying a file against its blob id

The blob id is the identity of the *uncompressed* bytes, so it can be checked
without the upstream repo at all:

```sh
gunzip -c product-12949-nordic-ultimate-omega.html.gz | git hash-object --stdin
# d4e7278c534b1806626927c68b0c49a3ad4fbc6d
```

To re-extract one from the fork:

```sh
git remote add caozhuozi https://github.com/caozhuozi/iherb-cli.git && git fetch caozhuozi
git cat-file blob <blob id> | gzip -9 -n > <slug>.html.gz
```

## Why each page is here

The legacy corpus first, in the order it was inherited, then the current one.
Every row says what that page uniquely exercises; a page that exercises nothing
another page does not is not worth 400 KB, and should be argued for or dropped.

| slug | why |
|---|---|
| `product-104996-cgn-two-a-day` | JSON-LD prices as a `priceSpecification` array with a strikethrough entry; a 29-row supplement table; **the only page with a populated review histogram**, which is what makes it the evidence for #32 |
| `product-108255-cgn-b-complex` | JSON-LD price as a flat top-level value; its review-histogram element is an empty 68-byte shell |
| `product-119174-olly-gummies` | the sparse one: out of stock, no review-histogram element at all, no `.prodOverviewIngred`, no overview sections |
| `product-12949-nordic-ultimate-omega` | softgels; the page the JS-globals side fixture was transcribed from |
| `product-59561-cgn-gold-c-powder` | powder; a one-nutrient supplement table; another empty histogram shell |
| `product-12949-nordic-ultimate-omega-nok` | the only non-USD page, and the only one captured by this repository: the same product as the row above, priced by the Norwegian storefront. It is what stops a currency sweep from passing by assuming one storefront, and it is where the `shipping_weight` tooltip rot on the current site is pinned |
| `search-vitamin-c` | 48 cards, 11,952 results, and the sort dropdown and category facets that #3 and #4 are about |
| `category-supplements` | nothing parses it yet; kept for the catalog command in #21 |

And the twelve from #8. What each one is *for* is a form or a field shape the
legacy corpus could not produce; the price and the currency they also carry are
covered by all thirteen NOK pages together and are nobody's individual reason
for being here.

| slug | why |
|---|---|
| `product-143499-biocidin-dentalcidin-toothpaste-nok` | **the corpus's only non-supplement, and its only page with no Supplement Facts panel.** A toothpaste sold by volume: no serving, no daily values, nothing to state them about. It is what broke `supplement_facts_parse_on_every_product_page`, and `golden/product-143499-full.md` is the only golden that shows the formatter omitting `## Nutrition` because the page has none rather than because something failed |
| `product-75722-dynamic-health-tart-cherry-nok` | **the only ingestible liquid.** `Package quantity` is `946 ml` — the corpus's only volume — and the serving size is `2 Tablespoons (30 ml)`, the only one measured in anything but units or grams |
| `product-118148-swanson-fiberaid-arabinogalactan-nok` | a powder sold by the gram, whose derived unit is kr/g. Its `Package quantity` is the bare string `250` — **no unit, no noun** — which is the counterexample to reading that field as "a number and a unit" |
| `product-16790-enzymedica-digest-gold-nok` | eleven enzymes dosed in **activity units** — DU, HUT, AGU, CU, FIP, ALU, GalU, SU, BGU, XU, HCU — and not one of them a mass. Anything that assumes a supplement-facts amount is `<number> <mass unit>` meets its counterexample here |
| `product-105890-bodybio-calcium-magnesium-butyrate-nok` | 250 capsules, **125 servings**: container count and serving count are different numbers on the same page, which #15 has to model as two things. It is also one of the two pages that spell the row `Serving Per Container`, singular |
| `product-4-doctors-best-r-lipoic-acid-nok` | product id **`4`** — one digit, against five and six everywhere else, so an id-shaped assumption is falsifiable for the first time. The other singular-spelling page, and the largest review count in the corpus at 8,555 |
| `product-78419-kal-lithium-orotate-nok` | **micro tablets** — a third unit noun, neither capsule nor tablet — and a flavour ("Lemon Lime") inside the product title. States no servings-per-container at all, in any spelling |
| `product-117699-swanson-supreme-c-complex-nok` | the corpus's first **tablet**, carrying a six-ingredient blend. Also states no servings-per-container in any spelling |
| `product-35060-arg-butyren-tributyrin-nok` | **delayed-release** veggie caps: the release mechanism is part of the form and the form string is where it is stated. Its serving size reads `1 Capsules` — the site's own plural-after-one, which is real markup and not a parse artefact |
| `product-12081-country-life-coenzyme-b-complex-nok` | a twelve-nutrient blend mixing mg, mcg and **mcg DFE** in one panel. Its servings-per-container is `120` for a 240-capsule bottle, because the serving is two capsules — the same container-vs-serving split as BodyBio, but here the page states both numbers |
| `product-132364-humanx-gasseri-reuteri-nok` | dosed in **CFU** — a count of live organisms, not a quantity of anything weighable |
| `product-124094-nutricost-k2-mk7-nok` | the **single-nutrient** page for contrast with the blends, dosed in micrograms. It is the weakest of the twelve on its own: what it uniquely holds is the contrast, not a shape no other page has. Kept because a corpus of edge cases with no ordinary member cannot say which is which |

## The JSON side-fixture

One parser is never fed page HTML and cannot be filled from these captures:

- `js-globals-12949.json` — `extract_js_globals` evaluates JS in the browser, so
  the loader cannot lift its result out of static HTML. Transcribed verbatim
  from the `window.PRODUCT_DETAILS` / `window.IHR_DL` block in the Nordic page.

There used to be two more, `next-data-*-synthetic.json`, written by hand to the
`__NEXT_DATA__` parsers' own documented expectations because no captured page
carried the blob. #34 deleted those parsers and their fixtures;
`product_json::next_data_is_absent_from_every_captured_page` remains as the
guard that says the absence still holds.

## `golden/`

Rendered Markdown, regenerated with `UPDATE_GOLDEN=1 cargo test` — exactly `1`,
any other value still asserts. Read the diff before committing a change to one.

`product-143499-full` is the first golden of a Norwegian page and the first of a
product that is not a dietary supplement. The `Shipping Weight` line in it
carries #51's tooltip text: that is characterized, not endorsed, and when #51
lands this golden changes and the diff is the proof.

**The goldens cover Markdown only.** #9 (`--json` output with a typed error
taxonomy) changes what callers consume and is not protected by anything here;
that wave must add its own snapshots or exact assertions, because a Markdown
golden will not notice a JSON schema change.

#28 (extraction provenance) has landed, and its assertions live in
`tests/parsers/provenance.rs` rather than in a golden — deliberately. A golden
pins a rendering; provenance is about where each value came from, which is
invisible in any rendering of the values. `health_serializes_to_the_block_issue_9_renders`
pins the JSON shape #9 has to emit.
