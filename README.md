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

1. User-configured path — `--browser-path`, then `IHERB_BROWSER_PATH`, then
   `browser_path` in the config file
2. System-installed Chrome/Chromium (auto-detected)
3. Auto-downloads [Chrome for Testing](https://googlechromelabs.github.io/chrome-for-testing/) on first run

```bash
iherb-cli product 12949 --browser-path /usr/bin/chromium
```

`--browser-path` is there because step 3 is a slow surprise. An agent that
cannot set an environment variable on a subprocess had no way to reach steps 1
or 2 and fell through to a download (#22).

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
| `--no-cache` | Touch the cache for neither reads nor writes | — |
| `--refresh` | Skip the cache on the way in, write the result on the way out | — |
| `--cache-ttl <duration>` | How long an entry stays usable: `30d`, `12h`, `45m`, `90s`, `2w` | `30d` |
| `--browser-path <path>` | Chrome or Chromium executable. Outranks `IHERB_BROWSER_PATH` and the config file | — |
| `--config <path>` | Read this config file instead of the one under the user's config dir | — |
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

# Re-read the page and keep the new answer
iherb-cli product 61864 --refresh

# A config file this run reads instead of ~/.config
iherb-cli product 61864 --config ./ci.toml
```

> **`--no-cache` changed meaning.** It used to disable cache *reads* and write
> the result anyway, so a caller asking not to touch the cache still got files
> on disk. That behaviour is now `--refresh`, and `--no-cache` does what its
> name says. There is no alias and no deprecation period: nothing has been
> released (#38), so the break costs no consumer anything.


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
    "requested_country": "no",
    "requested_currency": "NOK",
    "requested_storefront": "https://no.iherb.com"
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
            "from_cache": null, "requested_country": "no", "requested_currency": "NOK",
            "requested_storefront": "https://no.iherb.com" },
  "error_type": "cloudflare_blocked",
  "message": "Cloudflare challenge could not be solved after 3 attempts"
}
```

### What `meta` means

**`meta` describes the request. The record describes the answer.** Every
storefront field here is named `requested_` because that is what it is: iHerb
geolocates by IP and will override a `--country` you did not state, so with no
flags on a Norwegian IP the run resolves `us` and asks
`https://www.iherb.com` — and gets a price in NOK. A `meta` that called that
`country: "us"` and `currency: null` was describing the question while sitting
next to the answer.

- **`fetched_at`** is when the *page* was read; **`emitted_at`** is when the
  command ran. On a fresh fetch they are the same instant, and they are the
  same instant by construction: a fresh record's page was read during this run,
  so one clock sample dates both. On a cache hit they differ, and the difference
  is the whole point — a price read three weeks ago is not stale, it is wrong.
- **`from_cache`** says which of those two you are looking at.
- **`requested_country`, `requested_currency`, `requested_storefront`** are the
  values the run *resolved* to after flag → env → config file — not the flags as
  typed. A record produced by an unattended run with no flags at all still says
  what it asked for.
- **What the storefront actually answered is on the record, not here.** The
  currency a price is in is `data.currency`, and whether anybody read it off the
  page is `data.extraction.sources.currency`. The URL the record names is
  `data.product_url`, with `data.extraction.sources.product_url` saying whether
  the page published it or we defaulted it from the requested storefront. None
  of it is copied into `meta`: a value stored in two places is a value that can
  disagree with itself.
- `fetched_at` and `from_cache` are `null` when no page was read — a bad
  argument, a browser that would not start, an interrupt. A failure that happens
  *after* a page was read reports the page: `parse_failed` and
  `currency_mismatch` both mean the page loaded first.
- `requested_country`, `requested_currency` and `requested_storefront` are
  `null` when the run failed before its configuration resolved — an unparseable
  command line has no effective storefront, and naming one would be a claim
  about a run that never started. `requested_currency` is also `null` on a good
  run that passed no `--currency`, because then it asked for nothing in
  particular. See [what `--currency` does](#what---currency-does).

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

**Why version 1 covers two envelope shapes.** The first draft of `meta` named
its storefront fields `country`, `currency` and `storefront`; they are now
`requested_country`, `requested_currency` and `requested_storefront`. Three
fields removed and three added is a breaking change by the rule above, and it
happened without the version moving.

That is deliberate and it rests on a fact that is not in the code: **nothing has
been released.** The unprefixed names were on `origin` for about twenty minutes
and there is no tag and no published artefact, so no consumer can be holding a
document that has a `meta.country` in it. Bumping to 2 would spend this tool's
first version increment announcing a break from a shape nobody ever received,
and leave a row here describing a document that does not exist.

**The assumption expires at the first release.** From the first tag onwards any
change of this kind — a rename included, since a rename is a field removed and a
field added — bumps `schema_version` and adds a row here.

### Exit codes

`--json` or not, a run leaves on a stable code naming what happened. `0` is
success.

**Every code in this table has a producer.** A table that documents a
distinction the code cannot make is worse than no table: a caller branches on a
number that never arrives and never finds out. Four codes an earlier draft
carried — `network_error` (30), `io_error` (31), `cache_error` (32) and
`json_error` (40) — had no producer and have been removed rather than given a
decorative one. `reqwest` is used in exactly one place, so a network failure
there *is* a failed Chrome download; every filesystem failure sits inside an
operation that already has a code; the cache is an optimization, so a read that
fails is a miss and a write that fails is a log line; and a record of ours that
will not serialize is a bug in this tool, which is `internal_error`.

`cache_unreadable` (12), added with the `cache` command in #22, is not the
retired `cache_error` (32) under a new number. That code claimed an *incidental*
cache failure during a fetch could end a run, and none can — a full disk is
still a log line beside a perfectly good page. This one covers the case where
the caller asked a question *about the cache* and the directory will not open,
which is the only situation in which there is no honest answer to give. A
missing directory is an empty cache and exits 0; a single file `clear` cannot
remove is reported in the payload.

| exit | `error_type` | what it means | produced by |
|---|---|---|---|
| 2 | `invalid_input` | The arguments cannot produce a request. Fix them. | an empty query, `--limit 0`, an unknown `--category`, an identifier that is neither an id nor a URL, and an unknown `--country` |
| 10 | `browser_launch_failed` | Chrome would not start. The environment needs attention. | browser launch, including its temporary profile directory |
| 11 | `chrome_download_failed` | Chrome could not be obtained. | the first-run browser download: the version index, the transfer, the archive, and writing any of it to disk |
| 12 | `cache_unreadable` | The cache directory could not be listed, on a command whose job is to list it. | `cache stats` and `cache clear` against a directory that exists and will not open |
| 20 | `navigation_timeout` | The page did not load in time. Worth retrying. | a navigation the driver itself reported as a timeout |
| 21 | `navigation_failed` | The page did not load, and not because of the clock. | any other navigation failure |
| 22 | `cloudflare_blocked` | Cloudflare would not let us through. Retry later, from elsewhere. | the challenge loop giving up |
| 23 | `product_not_found` | iHerb says the product is gone. Stop asking about this id. | a 404 or not-found page |
| 24 | `empty_page_or_catalog_end` | The listing carried nothing. Not a fault. | a search whose result set is empty |
| 25 | `currency_mismatch` | The storefront prices in something else, and this tool does not convert. Change `--country`, or drop `--currency`. | `--currency` disagreeing with the price the storefront returned |
| 41 | `parse_failed` | **The page loaded and we could not read it.** The scraper is broken and a human should look. | a product page from which no strategy produced a name or a price |
| 70 | `internal_error` | This tool hit something it cannot name about itself. A bug. | anything unclassified, and a record of ours that will not serialize |
| 130 | `interrupted` | Ctrl+C. The browser was closed and its profile removed on the way out. | SIGINT during a command |

`parse_failed` and `internal_error` are deliberately separate. `parse_failed` is
the one code in this table worth alerting on — it means the site changed shape
under us — and it is worthless the moment every unrecognised error is filed
under it.

`currency_mismatch` is separate from `invalid_input` for the same kind of
reason. `--country us --currency CHF` is a perfectly well-formed command line:
it launches a browser, fetches a page, and can only fail once the storefront has
answered. A caller that reads `invalid_input` re-reads its arguments; a caller
that reads `currency_mismatch` changes what it expects of the storefront. Under
one code it had to parse `message` to tell which.

`interrupted` is in the table because `--json` carries **one document on stdout,
always** — and the interrupt used to be the exception that wrote none.

`--help` and `--version` are not command output: they print as usual and exit
`0`, `--json` or not.

## Configuration

Settings are resolved in order of priority:

1. CLI flags (highest)
2. Environment variables (`IHERB_BROWSER_PATH`, `IHERB_COUNTRY`, `IHERB_CURRENCY`)
3. Config file
4. Defaults

### Config file

Location: `~/.config/iherb-cli/config.toml`, or wherever `--config` points.

```toml
[defaults]
country = "ch"
# Optional. Same meaning as `--currency`: a requirement, not a conversion.
currency = "CHF"
# Optional. Same spelling as --cache-ttl.
cache_ttl = "12h"
# Optional. Outranked by --browser-path and IHERB_BROWSER_PATH.
browser_path = "/usr/bin/chromium"
delay_ms = 2000
```

`--config <path>` reads exactly that file. A path given there must exist and
must parse — a missing or malformed file is `invalid_input` (2), not a silent
fall-through to the defaults, because you asked for that file by name. The
default location stays optional, since most runs have no config file at all;
one that exists and will not parse is reported on stderr and then ignored.

## Caching

Scraped data is cached locally to reduce redundant requests. The cache directory is platform-dependent:

- **macOS:** `~/Library/Caches/iherb-cli/`
- **Linux:** `~/.cache/iherb-cli/`

Entries expire after **30 days** by default; `--cache-ttl` changes it:

```bash
iherb-cli product 12949 --cache-ttl 12h
```

One number for the whole record, deliberately. Thirty days is far too long for
a price and about right for a supplement facts panel — but both live in *one*
cached record, so telling them apart is a change to the model rather than a
second flag here. That is #15 and DECISION-01's work, not this flag's.

### Managing the cache

```bash
iherb-cli cache path                        # just the path, for $(...)
iherb-cli cache stats                       # entries, bytes, oldest, newest
iherb-cli cache clear --older-than 7d
iherb-cli cache clear --country no
iherb-cli cache clear --all
```

All three carry `--json` and answer in the same envelope as everything else,
with `meta.fetched_at` and `meta.from_cache` `null` — no page was read, which is
what those nulls already mean.

**`cache clear` deletes files, so here is exactly what it touches.** Regular
`.json` files sitting directly in the resolved cache directory: never a symlink,
never anything in a subdirectory, never anything outside it. `cache stats`
counts the same set, so what it reports is what `clear` can remove. Whatever was
removed is named in the report.

With no `--older-than` and no `--country` it removes everything, and that has to
be asked for with `--all`. There is no interactive prompt: this tool is run by
agents that cannot answer one, so a prompt would be a hang rather than a
safeguard.

**`--country` cannot reach search entries, and says so.** A product entry is
named `v4_product_<country>_<currency>_<id>.json` and is matched exactly. A
search entry is `v4_search_<hash>.json` — the country went into the hash and
cannot be read back off the name. Those entries are kept and counted, and the
report says how many, rather than letting "cleared the Norwegian cache" mean
something it does not. Clear them with `--older-than` or `--all`.

Per-data-kind TTLs are future work for the same reason the single TTL is: prices
and facts share a record. So is putting the country in a search entry's file
name, which would need a cache-generation bump and would abandon every entry
users already have.

### Freshness

Every Markdown document ends with a footer saying where its data came from and
when, set off from the body by a rule:

```markdown
---

*Data from the local cache, written 2026-08-23 12:34 UTC. Nothing was read from iHerb during this run.*
```

A fresh fetch says so instead: `*Data read from iHerb during this run, at
2026-09-01 12:34 UTC.*` Under `--json` the same two facts are `meta.fetched_at`
and `meta.from_cache`.

**The timestamp is when the page was read, not when the document was printed.**
That distinction only shows up on a cache hit, and on a cache hit it is the
whole point: a price read three weeks ago is not stale, it is wrong.

It is a footer rather than a bullet because a bullet belongs to whatever section
it follows, and this belongs to the document. `--section ingredients` used to
emit an `## Other Ingredients` block trailed by a top-level `- **Data from:**`
bullet that was part of nothing, and a section the page has no data for emitted
that bullet under no heading at all (#7).

**An absent section says when it was absent.** "No ingredients data available
for this product." on its own reads as something looked just now and found
nothing; off a cache hit, nothing looked at all. So it carries its own date:

```
No review data available for this product. That is what the cached record says,
and it was read on 2026-08-23 12:34 UTC — the page may have gained one since,
and nothing in this run went back to look.
```

Use `--no-cache` to bypass the cache and fetch fresh data.

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
