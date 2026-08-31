---
name: iherb-agent
description: Query supplement and health product data from iHerb using the iherb-cli command-line tool. Use when the user asks about supplements, vitamins, health products, or anything related to iHerb — including searching for products, comparing options, checking ingredients or nutrition facts, finding prices, reading review summaries, or getting purchase recommendations. Triggers include questions like "What's the best vitamin C?", "Find me a magnesium supplement", "What are the ingredients in this product?", "Compare omega-3 options on iHerb", or "How much does ashwagandha cost?".
---

# iherb-agent

Use the `iherb-cli` binary to query iHerb product data. It uses a headless browser (first run may take a moment to download Chrome). Results are cached for 30 days. Every result includes a `Data from:` timestamp — use `--no-cache` if the data is stale.

## Commands

### Search

```bash
iherb-cli search "<query>" [--limit <n>] [--sort <method>] [--category <slug>]
```

- `--limit`: max distinct products (default 20). iHerb repeats some products in a
  results grid; each is returned once, so N results means N different products.
  A run that comes up short says whether iHerb ran out of results or the walk did.
- `--sort`: `relevance` (default), `featured`, `best-selling`, `rating`, `most-rated`,
  `price-asc`, `price-desc`, `newest`, `highest-discount`. `relevance` is iHerb's
  ranking for the query; `featured` is its merchandised order, which is what the
  site shows when no sort is asked for. `most-rated` orders by number of reviews,
  which is usually what "well established" means — `rating` surfaces 5.0/5
  products with three reviews.
- `--category`: filter by category name (e.g. `supplements`, `vitamins`, `protein`,
  `herbs`, `minerals`, `sports`) or by a numeric iHerb category id (e.g. `1855`).
  A name the CLI does not know is an error and the message lists the ones it does,
  so a filter is never silently dropped.

Output: Markdown list with name, brand, price, rating, review count, product ID, URL.

### Product details

```bash
iherb-cli product <id-or-url> [--section <name>]
```

Accepts a numeric product ID (e.g., `61864`) or full URL.

`--section` options: `overview`, `description`, `ingredients`, `nutrition`, `suggested-use`, `warnings`, `reviews`

Output: Full Markdown with overview, supplement facts table, ingredients, suggested use, warnings, review distribution.

### Global flags

- `--country <code>`: localized storefront (e.g., `ch`, `de`, `jp`). Default: `us`
- `--currency <code>`: ask the storefront to price in this currency (e.g., `CHF`, `EUR`) and verify what came back. It does **not** convert — a storefront that prices in something else is an error, not a relabelling. Pass it whenever the currency matters: it is also what stops iHerb's IP geolocation overriding `--country`. Default: unset, which takes whatever iHerb serves
- `--json`: emit one JSON document on stdout instead of Markdown — see below
- `--no-cache`: bypass cache
- `--debug`: show browser window

## `--json`: reading this tool programmatically

Prefer `--json` over parsing the Markdown. stdout carries exactly one JSON
document, success or failure, and nothing else — logging goes to stderr.

```json
{ "ok": true, "schema_version": 1,
  "meta": { "tool_version": "0.1.1",
            "fetched_at": "2026-08-31T09:14:22Z", "emitted_at": "2026-08-31T11:02:05Z",
            "from_cache": true, "country": "no", "currency": "NOK",
            "storefront": "https://no.iherb.com" },
  "data": { } }
```

A failure has `"ok": false` and carries `error_type` and `message` where `data`
would be, in the same envelope.

Four things to know before acting on the output.

1. **`meta` is the record's provenance and it travels with it.** `fetched_at` is
   when the page was read, `emitted_at` is when the command ran; when
   `from_cache` is true they differ, and a price read weeks ago is not stale, it
   is wrong. `country`, `currency` and `storefront` are what the run resolved
   to, so a stored document is still interpretable on its own. Never compare two
   prices without comparing their `meta.storefront`.
2. **`meta.currency` is what the run *asked* for, not what a price is in.** The
   currency of a price is `data.currency`, and `null` there means the page
   published none. Do not substitute one for the other.
3. **`data.extraction` says whether to trust the record.** Same block on a
   product and on every search card. `degraded: true` means a field every
   product page publishes was not read, or a field the page carried could not be
   read — treat the numbers as suspect and say so. `fields_absent` is the page
   having nothing; `fields_defaulted` is a value nobody read off the page;
   `fields_malformed` is our parser failing on markup that was there.
4. **`null` is an answer.** `in_stock: null` means nothing on the page said
   either way — report that, do not report "in stock". `supplement_facts: null`
   means the product has no panel, which is normal for a non-supplement.

`--section` narrows the JSON exactly as it narrows the Markdown; `name`,
`product_id`, `product_url` and `extraction` are always present.

### Exit codes

Branch on the exit code rather than on the message text.

| exit | `error_type` | what to do |
|---|---|---|
| 0 | — | succeeded |
| 2 | `invalid_input` | fix the arguments — empty query, `--limit 0`, unknown `--category` or `--country`, an unusable product id, or a `--currency` this storefront does not price in |
| 10 / 11 | `browser_launch_failed`, `chrome_download_failed` | the environment is broken; tell the user, do not retry |
| 20 / 21 | `navigation_timeout`, `navigation_failed` | retry `20`; `21` usually means the URL is wrong |
| 22 | `cloudflare_blocked` | back off and retry later; do not hammer |
| 23 | `product_not_found` | the id is gone — stop asking about it |
| 24 | `empty_page_or_catalog_end` | the search matched nothing; try a different query |
| 30 / 31 / 32 / 40 | `network_error`, `io_error`, `cache_error`, `json_error` | local or transient; retry once, then report |
| 41 | `parse_failed` | **the page loaded and the scraper could not read it.** The site changed shape. Report it to the user as a tool bug, do not retry the same id in a loop |
| 70 | `internal_error` | a bug in the tool; report it with the message |
| 130 | — | interrupted |

`--help` and `--version` still print normally and exit `0` under `--json`.

## Workflows

### Find the best product for a need

1. Search with `--sort best-selling` or `--sort rating` to find top options
2. Get details on 2-3 top candidates: `iherb-cli product <id>`
3. Compare ingredients, dosage, price-per-serving, and ratings
4. Recommend with reasoning

```bash
iherb-cli search "vitamin d3" --limit 20 --sort best-selling
iherb-cli product 53330
iherb-cli product 18222
```

### Compare products

1. Get details for each product
2. Extract and compare: active ingredients, dosage per serving, servings per container, price per serving, rating, form (capsule/tablet/liquid)

```bash
iherb-cli product 53330 --section nutrition
iherb-cli product 18222 --section nutrition
```

Calculate price-per-serving: price / servings_per_container.

### Check specific product info

Use `--section` to fetch only what's needed:

```bash
iherb-cli product 61864 --section ingredients   # supplement facts + other ingredients
iherb-cli product 61864 --section nutrition      # supplement facts table only
iherb-cli product 61864 --section reviews        # rating breakdown
```

Note: `--section ingredients` returns both the supplement facts (active ingredients with amounts) and the other/inactive ingredients — everything you need in one call.

### Find budget options

```bash
iherb-cli search "magnesium glycinate" --sort price-asc --limit 20
```

Then verify quality by checking ingredients and ratings on the cheapest options.

### Localized pricing

Pass `--country` and `--currency` **together**. The pair is what iHerb honours:
it geolocates by IP, and a preference naming only a currency is discarded, so
`--country de` alone can still return whichever storefront your address suggests.

```bash
# The German storefront in euros, whatever the IP says
iherb-cli search "omega 3" --country de --currency EUR
iherb-cli product 61864 --country ch --currency CHF
```

Prices come back in the storefront's own currency; nothing is converted. A price
the page did not name a currency for is reported as
`(currency unknown: the page published none)` rather than being captioned with
one.

Never compare two prices without comparing their currencies. The same product is
NOK 880.63, €76.57 and $64.56 depending only on the storefront, so a number whose
currency is unknown is not comparable to one whose currency is known.

## Tips

- Search queries work best with specific supplement names (e.g., "magnesium glycinate" not just "magnesium")
- Always check the supplement facts table when comparing — brand marketing can be misleading
- Review count matters as much as rating — 4.5 stars with 10,000 reviews is more reliable than 5.0 with 12
- When recommending, mention form (capsule, tablet, powder, liquid), dosage, servings per container, and price per serving
- First run downloads a browser binary — warn the user it may take a moment
