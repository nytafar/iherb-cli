# Fixtures

Real iHerb pages, used by the parser tests in `tests/parsers/`. Each
`*.html.gz` is the page exactly as served, gzipped and renamed to an ASCII
slug — nothing is trimmed, because a trim would quietly discard the markup some
later bug turns out to be about. 20.5 MB of HTML compresses to 2.8 MB here.

## Where they came from

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

| slug | why |
|---|---|
| `product-104996-cgn-two-a-day` | JSON-LD prices as a `priceSpecification` array with a strikethrough entry; a 29-row supplement table; **the only page with a populated review histogram**, which is what makes it the evidence for #32 |
| `product-108255-cgn-b-complex` | JSON-LD price as a flat top-level value; its review-histogram element is an empty 68-byte shell |
| `product-119174-olly-gummies` | the sparse one: out of stock, no review-histogram element at all, no `.prodOverviewIngred`, no overview sections |
| `product-12949-nordic-ultimate-omega` | softgels; the page the JS-globals side fixture was transcribed from |
| `product-59561-cgn-gold-c-powder` | powder; a one-nutrient supplement table; another empty histogram shell |
| `search-vitamin-c` | 48 cards, 11,952 results, and the sort dropdown and category facets that #3 and #4 are about |
| `category-supplements` | nothing parses it yet; kept for the catalog command in #21 |

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

**The goldens cover Markdown only.** #9 (`--json` output with a typed error
taxonomy) changes what callers consume and is not protected by anything here;
that wave must add its own snapshots or exact assertions, because a Markdown
golden will not notice a JSON schema change.

#28 (extraction provenance) has landed, and its assertions live in
`tests/parsers/provenance.rs` rather than in a golden — deliberately. A golden
pins a rendering; provenance is about where each value came from, which is
invisible in any rendering of the values. `health_serializes_to_the_block_issue_9_renders`
pins the JSON shape #9 has to emit.
