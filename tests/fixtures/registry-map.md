# The registry, resolved to iHerb product ids

The 28 supplement notes in the user's registry that name iHerb as the supplier,
joined by hand to the products iHerb actually sells. Resolved 2026-09-01 against
the Norwegian storefront.

**This file is the join, and the join did not exist before it.** A registry note
records a brand, a form and a cost per unit:

```
**Brand**: Nutricost · **Form**: 240 capsules · **Cost**: 0.74 kr/cap · **Supplier**: [[registry/suppliers/iherb|iHerb]]
```

and **never an iHerb id or URL**, so there is nothing to follow. Every row below
was produced by searching for the brand and form and reading the results:

```sh
iherb-cli search "<brand> <product>" --country no --currency NOK --limit 8
```

That gap is `nytafar/hvelv#1`, and automating exactly this is what #43 `resolve`
is for. Until it exists, this table is the record — including the rows it could
not settle, which stay here marked rather than being dropped or guessed.

Nothing under `/Users/lasse/Vaults/` was modified. The registry was read.

## Confidence

| marker | means |
|---|---|
| **exact** | brand, form and unit count all match the note, and the current price per unit is within ~20% of the cost the note recorded |
| **likely** | brand and form match, but something the note records is missing, differently worded, or the note records too little to corroborate |
| **ambiguous** | several products fit, and the note does not record the one thing that would separate them. **Not resolved.** A best guess is named, and it is a guess |
| **unresolved** | no product on the storefront carries the brand the note records |

Price drift is not evidence of a wrong id on its own — the notes are from April
2026 and these prices are from September — so it is reported per row rather than
used as a tiebreak, except where a note records nothing else to go on.

## The table

| note | brand (as the note states it) | form (as the note states it) | cost (note) | id | title (as iHerb returns it) | now | confidence |
|---|---|---|---|---|---|---|---|
| `acetyl-l-carnitine` | Nutricost | 500 g Powder | 1.28 kr/g | `148505` | Nutricost, Acetyl L-Carnitine, Unflavored, 1.1 lb (500 g) | 1.24 kr/g | **exact** |
| `agmatine-sulfate` | Nutricost | 100 g Powder | 1.87 kr/g | `131992` | Nutricost, Agmatine, Unflavored, 3.6 oz (100 g) | 1.83 kr/g | **likely** — iHerb titles it "Agmatine", the note "Agmatine Sulfate". The sulfate is the salt it is sold as, and this is the only Nutricost agmatine powder at 100 g. `145307` is a Nutricost "Agmatine Sulfate" but is 120 *capsules* |
| `arabinogalactan` | Swanson FiberAid | 250 g Powder | 1.18 kr/g | `118148` | Swanson Vitamins, FiberAid® Larch Tree Arabinogalactan (AG), 8.8 oz (250 g) | 1.11 kr/g | **exact** |
| `astaxanthin` | Nutricost | 120 softgels, 6 mg/softgel | — | `132290` | Nutricost, Astaxanthin, 6 mg, 120 Softgels | 2.39 kr/softgel | **exact** |
| `b-complex` | Country Life | 240 capsules | 2.32 kr/cap | `12081` | Country Life, Coenzyme B-Complex, 240 Vegan Capsules | 2.17 kr/cap | **exact** |
| `boron` | Nutricost | 240 capsules | 0.89 kr/cap | — | — | — | **ambiguous** — see below |
| `calcium-magnesium-butyrate` | BodyBio | 250 capsules (125 servings) | 2.79 kr/cap | `105890` | BodyBio, Calcium Magnesium Butyrate, 250 Capsules | 2.60 kr/cap | **exact** |
| `coq10` | Lake Avenue Nutrition | 360 veggie caps | 1.52 kr/cap | — | — | — | **unresolved** — see below |
| `dentalcidin` | Biocidin Botanicals | 90 ml tube | — | `143499` | Biocidin Botanicals, Dentalcidin®, Oral Microbiome Toothpaste, Natural Mint, 3 oz (90 ml) | NOK 300.55 | **exact** |
| `digestive-enzymes` | Enzymedica | 240 capsules | 4.92 kr/cap | `16790` | Enzymedica, Digest Gold® with ATPro™, 240 Capsules | 4.50 kr/cap | **exact** |
| `lactobacillus-gasseri-reuteri` | Humanx | 30 capsules | 10.84 kr/cap | `132364` | Humanx, Lactobacillus Gasseri & Reuteri+, 30 Veggie Capsules | 10.12 kr/cap | **exact** |
| `lactobacillus-reuteri` | Vitamatic | 100 g Powder | 5.81 kr/g | `147583` | Vitamatic, Lactobacillus Reuteri, 3.5 oz (100 g) | 6.74 kr/g | **exact** — brand and form both unique on the storefront; price up 16% |
| `lithium-orotate` | KAL | 90 micro tablets | 1.17 kr/tablet | `78419` | KAL, Lithium Orotate, Lemon Lime, 5 mg, 90 Micro Tablets | 1.09 kr/tablet | **exact** — the only KAL lithium orotate in *micro tablets*; `70121`/`85528` are VegCaps |
| `milk-thistle` | Nutricost, Organic | **not recorded** | **not recorded** | `147076` | Nutricost, Organic Milk Thistle, Unflavored, 8.1 oz (227 g) | NOK 337.48 | **likely** — the only Nutricost organic milk thistle on the storefront, but the note records neither form nor cost, so nothing corroborates it |
| `phosphatidylcholine` | Natural Factors | 90 softgels, 420 mg/softgel | — | `2649` | Natural Factors, Phosphatidyl Choline (PC), 420 mg, 90 Softgels | NOK 164.46 | **exact** |
| `phosphatidylserine` | Probase Nutrition | 120 capsules, 150 mg/cap | — | `148530` | Probase Nutrition, Phosphatidyl Serine, 120 Capsules (150 mg per Capsule) | NOK 249.14 | **exact** |
| `pregnenolone` | Nutricost | 120 capsules | 1.44 kr/cap | — | — | — | **ambiguous** — see below |
| `psyllium-husk` | Frontier Co-op | 453 g Powder, Organic | 0.53 kr/g | `31080` | Frontier Co-op, Organic Psyllium Husk Powder, 16 oz (453 g) | 0.77 kr/g | **exact** — "Powder" and "Organic" together pick it out; `30981` is whole husk, `30709` is non-organic. Price up 45% |
| `quercetin` | Natural Factors | 60 liquid softgels | 5.58 kr/softgel | `101704` | Natural Factors, Quercetin LipoMicel Matrix, 250 mg, 60 Liquid Softgels | 5.21 kr/softgel | **exact** |
| `r-lipoic-acid` | Doctor's Best | 60 veggie caps | 4.26 kr/cap | `4` | Doctor's Best, Stabilized R-Lipoic Acid, 100 mg, 60 Veggie Caps | 4.33 kr/cap | **exact** — the 200 mg/60 (`43211`) is 6.89 kr/cap, far outside |
| `selenium` | Nutricost | 240 capsules | 0.74 kr/cap | — | — | — | **ambiguous** — see below |
| `spm` | Life Extension | 30 softgels | 7.86 kr/softgel | `118671` | Life Extension, Pro-Resolving Mediators, 30 Softgels | 7.18 kr/softgel | **exact** |
| `stinging-nettle-leaf` | Swanson | 120 capsules, 400 mg/cap | — | `117667` | Swanson Vitamins, Stinging Nettle Leaf, 400 mg, 120 Capsules | NOK 95.12 | **exact** — leaf, not root (`109033`) and not the unspecified `117985` |
| `tart-cherry-concentrate` | Dynamic Health | 946 ml Liquid | — | `75722` | Dynamic Health, Tart Cherry Concentrate, 32 fl oz (946 ml) | NOK 408.53 | **exact** — the 473 ml bottles (`130758`, `23919`) are the same product in half the size |
| `trehalose` | Swanson | 454 g Powder | 1.08 kr/g | `118145` | Swanson Vitamins, Trehalose, 1 lb (454 g) | 0.50 kr/g | **exact** on brand and form — the only Swanson trehalose there is. But the price is **less than half** what the note recorded, which is the largest divergence in this table and is flagged rather than explained |
| `tributyrin` | Allergy Research Group | 100 delayed-release veggie caps | 3.13 kr/cap | `35060` | Allergy Research Group, ButyrEn®, 100 Vegetarian Capsules (200 mg per Capsule) | 2.86 kr/cap | **exact** |
| `vitamin-c-complex` | Swanson | 250 tablets | 1.31 kr/tablet | `117699` | Swanson Vitamins, Supreme C-Complex with Citrus Bioflavonoids and Rutin, 250 Tablets | 1.22 kr/tablet | **exact** |
| `vitamin-k2` | Nutricost | 240 softgels | 0.87 kr/softgel | `124094` | Nutricost, Vitamin K2 MK-7, 100 mcg, 240 Softgels | 0.83 kr/softgel | **exact** |

22 exact, 2 likely, 3 ambiguous, 1 unresolved.

## The four that are not resolved

### `boron`, `pregnenolone`, `selenium` — the same failure, three times

All three notes record the container (240 or 120 capsules) and the cost per
capsule, and record the dose as **`— (label incomplete)`**. Nutricost sells each
of these in several strengths *in the same container size at a similar price*,
so the note contains nothing that separates them. Cost per capsule is the only
discriminator left, and it is a four-month-old price against a current one.

| note | recorded | candidates (240 or 120 caps) | nearest on price |
|---|---|---|---|
| `boron` | 0.89 kr/cap | `128946` 10 mg → 0.87 · `128914` 5 mg → 0.82 · `132258` 3 mg → 0.77 | `128946` (10 mg) |
| `pregnenolone` | 1.44 kr/cap | `135559` 30 mg → 1.42 · `132263` 50 mg → 1.51 · `132252` 10 mg → 1.17 · `135654` 100 mg → 1.90 | `135559` (30 mg) |
| `selenium` | 0.74 kr/cap | `141690` 200 mcg → 0.72 · `136863` 100 mcg → 0.67 | `141690` (200 mcg) |

The gaps between candidates (0.05–0.09 kr/cap) are smaller than four months of
ordinary price movement — `psyllium-husk` moved 0.24 kr/g over the same period,
and `trehalose` moved 0.58. **A price tiebreak at this resolution is noise.**
Two of these three doses also matter clinically: selenium's safe range is
narrow, and boron at 10 mg is near the top of the therapeutic range the note
itself quotes.

**Not captured as fixtures, and the ids above should not be copied anywhere as
if they were resolved.** What settles them is the label, which is in the
physical bottle and not on any page this tool can read.

### `coq10` — the brand is gone

The note records **Lake Avenue Nutrition, 360 veggie caps, 1.52 kr/cap**.

Lake Avenue Nutrition still exists on the Norwegian storefront — it returns
L-Serine (`123749`) and an energy powder (`108263`) — but it sells **no CoQ10**
there now. The product was delisted, renamed, or moved between iHerb's house
brands since April.

Two California Gold Nutrition products fit the form and the price band:

| id | title | kr/cap |
|---|---|---|
| `95215` | California Gold Nutrition, CoQ10, USP Grade Ubiquinone, 100 mg, 360 Veggie Capsules | 1.43 |
| `91611` | California Gold Nutrition, CoQ10, Ubiquinone USP with Bioperine®, 100 mg, 360 Veggie Capsules | 1.30 |

`95215` matches the note's description ("USP-grade ubiquinone", no mention of a
bioavailability enhancer) almost word for word, and Lake Avenue Nutrition and
California Gold Nutrition are both iHerb house brands, so a rebrand is the
likeliest story. **It is still a story.** Neither product carries the brand the
note records, and this row stays unresolved until someone with the bottle says
otherwise.

## Two things about the registry itself

Neither is a defect this repository can or should fix — the vault is a different
repository and the user's own files — but both affect anyone repeating this join.

**The notes use two formats, and both are well-formed.** Nine notes put the
product info in a bulleted list under `## Product Info`; nineteen put it on one
line as `**Brand**: X · **Form**: Y · **Cost**: Z · **Supplier**: …`. The second
looks run-together next to the first but parses cleanly and states the same
fields. No note was unreadable, and no brand had to be guessed from prose.

**The supplier wikilink is written two ways.** Nine notes link
`[[suppliers/iherb|iHerb]]` and nineteen link `[[registry/suppliers/iherb|iHerb]]`.
A query matching only one of them finds a third of the corpus. This join matched
on the substring `suppliers/iherb`, which catches both.

## What was captured

Twelve of these ids became fixtures. Which twelve, and why each, is in
`README.md` under "The current corpus". The selection was made to span **forms**
— capsule, softgel, tablet, micro tablet, delayed-release veggie cap, powder by
the gram, liquid by the millilitre — because that is what #15's structured
quantity and container model has to be designed against, and the corpus before
#8 had none of the last four.
