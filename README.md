# iherb-cli

A Rust command-line tool for querying product data from [iHerb](https://www.iherb.com). Designed for both AI agents and humans — clean commands, Markdown output, no API key required.

iHerb has no official API. This CLI uses a headless browser to load pages (bypassing Cloudflare), extracts structured data, and presents it in a consistent, parseable format.

## Installation

```bash
curl -fsSL https://raw.githubusercontent.com/nytafar/iherb-cli/master/install.sh | bash
```

This downloads the latest release binary for your platform and installs it to `/usr/local/bin`. Run the same command again to update.

### Build from source

Requires [Rust](https://www.rust-lang.org/tools/install) (1.70+).

```bash
git clone https://github.com/nytafar/iherb-cli.git
cd iherb-cli
cargo build --release
```

The binary will be at `target/release/iherb-cli`.

### Browser

iherb-cli needs a Chromium-based browser. It resolves one automatically:

1. User-configured path (`IHERB_BROWSER_PATH` env var or config file)
2. System-installed Chrome/Chromium (auto-detected)
3. Auto-downloads [Chrome for Testing](https://googlechromelabs.github.io/chrome-for-testing/) on first run

## Usage

```
iherb-cli <command> [options] [arguments]
```

### Search for products

```bash
iherb-cli search "vitamin c"
iherb-cli search "omega 3" --limit 20 --sort price-asc
iherb-cli search "protein" --category supplements --sort best-selling
```

**Options:**

| Flag | Description | Default |
|---|---|---|
| `--limit <n>` | Max **distinct products** to return (paginates automatically). iHerb places some products twice on a results page; those count once. A result short of the limit says whether iHerb ran out or the walk did. | 20 |
| `--sort <method>` | `relevance`, `featured`, `best-selling`, `rating`, `most-rated`, `price-asc`, `price-desc`, `newest`, `highest-discount` | `relevance` |
| `--category <slug\|id>` | Filter by category name (`supplements`, `vitamins`, `protein`, …) or numeric iHerb category id (`1855`). An unknown name is an error, not a silent no-op; the message lists the names that work. | — |

**Example output:**

```markdown
## Search results for "vitamin c" (showing 3 of 1,200+)

### 1. California Gold Nutrition, Gold C, Vitamin C, 1,000 mg, 240 Veggie Capsules
- **Brand:** California Gold Nutrition
- **Price:** $9.60 ~~$12.00~~
- **Rating:** 4.6/5 (12,345 reviews)
- **ID:** 61864
- **URL:** https://www.iherb.com/pr/california-gold-nutrition-gold-c-1000-mg-240-veggie-capsules/61864

---

### 2. Now Foods, C-1000, 250 Tablets
- **Brand:** Now Foods
- **Price:** $11.85
- **Rating:** 4.7/5 (8,901 reviews)
- **ID:** 479
- **URL:** https://www.iherb.com/pr/now-foods-c-1000-250-tablets/479
```

### Get product details

```bash
iherb-cli product 61864
iherb-cli product https://www.iherb.com/pr/some-product/61864
iherb-cli product 61864 --section ingredients
```

Accepts a numeric product ID or a full iHerb URL.

**Options:**

| Flag | Description |
|---|---|
| `--section <name>` | Show only one section: `overview`, `description`, `ingredients`, `nutrition`, `suggested-use`, `warnings`, `reviews` |

**Example output:**

```markdown
# California Gold Nutrition, Gold C, Vitamin C, 1,000 mg, 240 Veggie Capsules

## Overview
- **Brand:** California Gold Nutrition
- **Price:** $9.60 ~~$12.00~~ (20% off)
- **Rating:** 4.6/5 (12,345 reviews)
- **Availability:** In Stock
- **Product Code:** CGN-01065

## Supplement Facts
| Nutrient | Amount | % Daily Value |
|---|---|---|
| Vitamin C (as L-ascorbic acid) | 1,000 mg | 1,111% |

- **Serving Size:** 1 Veggie Capsule
- **Servings Per Container:** 240

## Suggested Use
Take 1 capsule daily with or without food.
```

### Global flags

| Flag | Description | Default |
|---|---|---|
| `--country <code>` | Country code for localized pricing (e.g., `us`, `ch`, `de`). On its own, iHerb may still override it from your IP | `us` |
| `--currency <code>` | Ask the storefront to price in this currency (e.g., `USD`, `CHF`, `EUR`), and check what came back. Does **not** convert — see below | none |
| `--no-cache` | Bypass local cache and fetch fresh data | — |
| `--delay <ms>` | Delay between requests in milliseconds | `2000` |
| `--json` | Emit one JSON document on stdout instead of Markdown — see below | — |
| `--debug` | Run the browser headed (a visible window), log at debug level, and print the provenance table | — |

```bash
# Swiss storefront in Swiss francs, whatever your IP says you are
iherb-cli search "magnesium" --country ch --currency CHF

# Fast mode (shorter delay between requests)
iherb-cli search "zinc" --delay 500

# Debug with visible browser
iherb-cli product 61864 --debug
```

### What `--currency` does

It asks, then checks. It never converts.

iHerb prices in the currency of the storefront you are on, and it carries that
choice in the cookies its own header picker writes. `--currency` sets them
before the page is fetched, so the storefront really does price in what you
asked for; the currency is then read back off the page, and a storefront that
priced in something else is an error naming both. Nothing is converted and no
price is captioned with a currency the page did not publish.

So the same product genuinely differs by storefront — measured on 2026-08-31:

```
iherb-cli product 12949 --country no --currency NOK   # NOK 880.63
iherb-cli product 12949 --country de --currency EUR   # €76.57
iherb-cli product 12949 --country us --currency USD   # $64.56
```

**`--currency` is also what makes `--country` stick.** iHerb geolocates by IP,
and the preference it honours names a country *and* a currency together — a
currency on its own is discarded. Without `--currency`, `--country us` from a
Norwegian address returns the Norwegian storefront in NOK. With it, you get the
storefront you named.

Omit it — the default — to take whatever iHerb serves.

A price whose currency the page did not publish is reported as
`9.60 (currency unknown: the page published none)`, never as `$9.60` or as the
currency you asked for. A number captioned with the wrong currency is worse than
a number with none: an agent comparing `CHF 9.60` against `CHF 12.00` from a
real Swiss storefront produces a confidently wrong recommendation.

## JSON output

`--json` replaces the Markdown with exactly one JSON document on stdout. Success
or failure, cached or fresh, it is always one document and it is always the only
thing on stdout — logging goes to stderr.

```bash
iherb-cli product 12949 --country no --currency NOK --json
```

```json
{
  "ok": true,
  "schema_version": 1,
  "meta": {
    "tool_version": "0.1.1",
    "fetched_at": "2026-08-31T09:14:22Z",
    "emitted_at": "2026-08-31T11:02:05Z",
    "from_cache": true,
    "country": "no",
    "currency": "NOK",
    "storefront": "https://no.iherb.com"
  },
  "data": {
    "name": "Nordic Naturals, Ultimate Omega",
    "price": 880.63,
    "currency": "NOK",
    "in_stock": null,
    "supplement_facts": { },
    "extraction": {
      "strategy": "json_ld",
      "enriched": true,
      "sources": { "name": "json_ld", "in_stock": "absent", "currency": "json_ld" },
      "fields_absent": ["in_stock"],
      "fields_defaulted": [],
      "fields_malformed": ["review_distribution"],
      "degraded": true
    }
  }
}
```

A failure wears the same envelope, with `error_type` and `message` where `data`
would be:

```json
{
  "ok": false,
  "schema_version": 1,
  "meta": { "tool_version": "0.1.1", "fetched_at": null, "emitted_at": "2026-08-31T11:02:05Z",
            "from_cache": null, "country": "no", "currency": "NOK",
            "storefront": "https://no.iherb.com" },
  "error_type": "cloudflare_blocked",
  "message": "Cloudflare challenge could not be solved after 3 attempts"
}
```

### What `meta` means

- **`fetched_at`** is when the *page* was read; **`emitted_at`** is when the
  command ran. On a fresh fetch they are the same instant. On a cache hit they
  are not, and the difference is the whole point: a price read three weeks ago
  is not stale, it is wrong.
- **`from_cache`** says which of those two you are looking at.
- **`country`, `currency`, `storefront`** are the values the run *resolved* to
  after flag → env → config file — not the flags as typed. A record produced by
  an unattended run with no flags at all still says which storefront it came
  from.
- `fetched_at` and `from_cache` are `null` when no page was read, which is every
  failure. `country`, `currency` and `storefront` are `null` when the run failed
  before its configuration resolved — an unparseable command line has no
  effective storefront, and naming one would be a claim about a run that never
  started.
- **`meta.currency` is the currency the run asked the storefront for**, and is
  `null` when it asked for nothing in particular. The currency a *price* is in
  is `data.currency`, and whether anybody read it off the page is in
  `data.extraction.sources.currency`. These are deliberately two different
  fields: see [what `--currency` does](#what---currency-does).

### `data.extraction`

Every record — a product, and every card in a search result — carries the same
provenance block: which strategy produced it, where each field came from, and
whether the record looks degraded. `absent` means the page published nothing;
`defaulted` means there is a value nobody read off the page; `malformed` means
the page carried the field and the parser could not read it. `degraded` is the
"our selectors may have rotted" signal, and a caller acting on price data should
check it.

`null` is a value in this output, not an omission. `in_stock: null` means no
signal on the page said either way, which is a different answer from `false`;
`supplement_facts: null` means the product has no Supplement Facts panel, which
plenty of them genuinely do not.

`--section` narrows the document the same way it narrows the Markdown. The
record's identity (`name`, `product_id`, `product_url`) and its `extraction`
block are always present, whatever section was asked for.

### Schema versions

`schema_version` is bumped only on a **breaking** change to `data`: a field
removed, a field re-typed, or a field whose meaning changes. Purely additive
fields do not bump it, so a consumer that ignores unknown keys keeps working
across additions.

| version | what changed |
|---|---|
| 1 | Initial `--json` output: the `ok` / `schema_version` / `meta` / `data` envelope, product and search payloads, and the `extraction` provenance block on both. |

### Exit codes

`--json` or not, a failure exits on a stable code naming what went wrong. `0` is
success and `130` is an interrupt (Ctrl+C).

| exit | `error_type` | what it means | produced by |
|---|---|---|---|
| 2 | `invalid_input` | The arguments cannot produce a request. Fix them. | an empty query, `--limit 0`, an unknown `--category`, an identifier that is neither an id nor a URL, an unknown `--country`, and a `--currency` the storefront does not price in |
| 10 | `browser_launch_failed` | Chrome would not start. The environment needs attention. | browser launch |
| 11 | `chrome_download_failed` | Chrome could not be downloaded. | the first-run browser download |
| 20 | `navigation_timeout` | The page did not load in time. Worth retrying. | a navigation failure whose cause names a timeout |
| 21 | `navigation_failed` | The page did not load, and not because of the clock. | any other navigation failure |
| 22 | `cloudflare_blocked` | Cloudflare would not let us through. Retry later, from elsewhere. | the challenge loop giving up |
| 23 | `product_not_found` | iHerb says the product is gone. Stop asking about this id. | a 404 or not-found page |
| 24 | `empty_page_or_catalog_end` | The listing carried nothing. Not a fault. | a search whose result set is empty |
| 30 | `network_error` | The network failed under us. | the Chrome download's HTTP client |
| 31 | `io_error` | The filesystem failed under us. | file reads and writes |
| 32 | `cache_error` | The cache could not be read or written. | the cache directory |
| 40 | `json_error` | JSON we produced or consumed would not round-trip. | cache serialization |
| 41 | `parse_failed` | **The page loaded and we could not read it.** The scraper is broken and a human should look. | a product page from which no strategy produced a name or a price |
| 70 | `internal_error` | This tool hit something it cannot name about itself. A bug. | anything unclassified |

`parse_failed` and `internal_error` are deliberately separate. `parse_failed` is
the one code in this table worth alerting on — it means the site changed shape
under us — and it is worthless the moment every unrecognised error is filed
under it.

`--help` and `--version` are not command output: they print as usual and exit
`0`, `--json` or not.

## Configuration

Settings are resolved in order of priority:

1. CLI flags (highest)
2. Environment variables (`IHERB_BROWSER_PATH`, `IHERB_COUNTRY`, `IHERB_CURRENCY`)
3. Config file
4. Defaults

### Config file

Location: `~/.config/iherb-cli/config.toml`

```toml
[defaults]
country = "ch"
# Optional. Same meaning as `--currency`: a requirement, not a conversion.
currency = "CHF"
```

## Caching

Scraped data is cached locally to reduce redundant requests. The cache directory is platform-dependent:

- **macOS:** `~/Library/Caches/iherb-cli/`
- **Linux:** `~/.cache/iherb-cli/`

All cached data expires after **30 days**.

Every result includes a `Data from:` timestamp so you know how fresh the data is. Use `--no-cache` to bypass the cache and fetch fresh data.

## How it works

iHerb uses Cloudflare anti-bot protection, so simple HTTP requests are blocked. iherb-cli uses a headless Chromium browser (via the Chrome DevTools Protocol) to load pages like a real user.

**Data extraction** for a product page uses three strategies with automatic
fallback:

1. **JSON-LD** structured data embedded in the page — this is the one that
   fires today, on every page we have looked at
2. **JavaScript globals** (`window.IHR_DL.product`, `window.PRODUCT_DETAILS`)
3. **DOM scraping** — CSS selector-based extraction as a last resort

Whichever wins, the result is then enriched from the DOM, so the fields you get
do not depend on which strategy got there first.

A search page has one strategy: read the product cards out of the DOM.

**Every field records where it came from** — JSON-LD, JS globals, the DOM, or
absent, meaning every strategy ran and none produced it. That distinction is
why the tool can tell "iHerb publishes no shipping weight for this item" from
"our selector rotted": both used to be an empty field and nothing could tell
them apart. `--debug` prints the whole provenance table alongside the product,
and a record missing something every product page publishes prints a
`Data quality: degraded` line whether you asked for it or not.

For the same reason, a page that loads and yields nothing is reported as a
failed extraction rather than a missing product. The first means "retry is
reasonable and a human should look at the selectors"; the second means "stop
asking about this id". Reporting the first as the second is the worst available
answer, and it is what the tool used to do.

There used to be a fourth product rung, `__NEXT_DATA__`. iHerb does not serve
that blob — not on any of the seven captured pages, and not on pages fetched
live in August 2026 — so it was never a fallback, only the appearance of one.
It is deleted rather than left in place, because an advertised fallback that has
never fired is worse than an honest shorter chain.

## Claude Code skill

This repo includes a [Claude Code skill](https://code.claude.com/docs/en/skills) that teaches AI agents how to use `iherb-cli` for supplement research. With the skill installed, Claude can autonomously search for products, compare ingredients, and make recommendations.

### Install the skill

```bash
/install-plugin iherb-agent@nytafar/iherb-cli
```

### What the agent can do

Once installed, Claude can handle requests like:

- *"What's the best-rated vitamin D3 on iHerb?"*
- *"Compare the top 3 magnesium glycinate supplements by price and dosage"*
- *"What are the ingredients in iHerb product 61864?"*
- *"Find me a budget omega-3 supplement with good reviews"*

The skill guides Claude through multi-step workflows — searching, fetching details, comparing nutrition facts, and recommending products with reasoning.

### Requirements

The `iherb-cli` binary must be available on `PATH`. Build it first:

```bash
cargo build --release
export PATH="$PATH:$(pwd)/target/release"
```

## Supported countries

45+ country codes including: `us`, `ca`, `au`, `nz`, `gb`, `de`, `fr`, `ch`, `at`, `it`, `es`, `nl`, `be`, `se`, `no`, `dk`, `fi`, `jp`, `kr`, `cn`, `tw`, `hk`, `sg`, `my`, `th`, `in`, `ae`, `sa`, `il`, `br`, `mx`, `cl`, `co`, `ar`, and more.
