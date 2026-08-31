# Fixtures

Real iHerb pages, captured 2025, used by the parser tests in `tests/parsers/`.

Each `*.html.gz` is the page exactly as served, gzipped and renamed to an ASCII
slug — nothing is trimmed, because a trim would quietly discard the markup some
later bug turns out to be about. 20.5 MB of HTML compresses to 2.8 MB here.

They come from the `caozhuozi/iherb-cli` fork, which committed them uncompressed
under `fixtures/`. To re-extract one:

```sh
git show 'caozhuozi/master:fixtures/<original name>' | gzip -9 -n > <slug>.html.gz
```

| file | page | why it is here |
|---|---|---|
| `product-104996-cgn-two-a-day.html.gz` | CGN Multivitamin Two-A-Day | JSON-LD prices arrive as a `priceSpecification` array with a strikethrough entry; 29-row supplement table |
| `product-108255-cgn-b-complex.html.gz` | CGN High Potency B Complex | JSON-LD price is a flat top-level value |
| `product-119174-olly-gummies.html.gz` | OLLY Goodbye Stress gummies | the sparse one: out of stock, no review histogram, no `.prodOverviewIngred`, no overview sections |
| `product-12949-nordic-ultimate-omega.html.gz` | Nordic Naturals Ultimate Omega | softgels; the page the JS-globals side-fixture was transcribed from |
| `product-59561-cgn-gold-c-powder.html.gz` | CGN Gold C Powder | powder; a one-nutrient supplement table |
| `search-vitamin-c.html.gz` | `/search?kw=vitamin+c` | 48 cards, 11,952 results, and the sort dropdown and category facets that #3 and #4 are about |
| `category-supplements.html.gz` | `/c/supplements` | nothing parses it yet; kept for the catalog command in #21 |

## The JSON side-fixtures

Two parsers are never fed page HTML, and neither can be filled from these
captures:

- `js-globals-12949.json` — `extract_js_globals` evaluates JS in the browser, so
  the loader cannot lift its result out of static HTML. Transcribed verbatim
  from the `window.PRODUCT_DETAILS` / `window.IHR_DL` block in the Nordic page.
- `next-data-*-synthetic.json` — **synthetic.** None of the seven pages contains
  a `__NEXT_DATA__` block; iHerb does not serve one. These are written to the
  `__NEXT_DATA__` parsers' own documented expectations so those parsers have an
  input at all. `product_json::next_data_is_absent_from_every_captured_page`
  pins the absence.

## `golden/`

Rendered Markdown, regenerated with `UPDATE_GOLDEN=1 cargo test`. Read the diff
before committing a change to one.
