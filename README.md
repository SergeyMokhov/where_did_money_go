# where-did-money-go

A bank / credit / debit **statement analyzer** that ingests CSV exports from multiple
banks, cards, and people, then produces a single **self-contained interactive report**
answering: *where did the money come from, where did it go, and how can I save?*

It comes three ways, all running the same Rust engine: an **offline browser app** (the engine
compiled to WebAssembly), a [Claude Code](https://claude.com/claude-code) **skill**, and a
plain Rust CLI (`wdmg`).

**▶ [Live demo](https://sergeymokhov.github.io/where_did_money_go/)** — the browser app,
opened on the bundled sample data. No install, no sign-up, no server: it is one HTML file and
contacts no network. Under **⚙ Configuration** you can create a project from your own CSV
exports — they are analyzed in the page and stored only in your browser. The sample household
(the *Futurama* crew, year 2999) keeps a few secrets buried in the numbers — slice by person,
search the merchants, and follow the ⓘ hints to see if you can work out *where Bender's money
really went* before the report spells it out.

> The shipped example configuration is themed (Planet Express, New New York, …). It is a
> **template** — replace it with your own. Your real, private config should live **outside
> this repo** (see [Privacy](#privacy)).

---

## What it does

- **Reads many export formats** (auto-detected): a cardholder credit card, a rewards credit
  card, a generic bank ledger (chequing/savings), a fixed-width card statement, and a ledger
  whose description is split over several columns. New banks are added by editing config —
  **no recompile** — or, in the browser app, with **Map columns…**.
- **Deduplicates** overlapping exports (e.g. a full-history file plus a recent-only file) so
  nothing is double-counted; keeps legitimate same-day duplicates within one file.
- **Categorizes** every transaction by configurable rules (most-specific match wins, optional
  amount ranges per rule, one-off transactions pinned by hand), with a
  `transfer` kind that nets credit-card payments and inter-account moves out of both flows —
  fixing the classic "my card payment looks like spending" double-count.
- **Models a household**: people own multiple cards/accounts; **external accounts** (e.g. a
  savings account that never appears in the exports) are valid transfer destinations.
- **Reconciles cross-account / cross-person transfers**: pairs the two legs of an internal
  move, assigns one-legged transfers to a person/account, and renders a money-flow graph.
- **Analytics** (all exact-decimal, deterministic): monthly/weekly flow in/out, spend by
  category, income by source, savings rate, **recurring/subscription detection**, **anomaly
  detection** (robust MAD z-score + monthly IQR), trends, top merchants, per-person/-account
  breakdowns, and a dedup report.
- **Interactive offline report**: opens from `file://` with no server and no network. The
  charting library is inlined; there are zero external requests.

### Where the LLM fits
The Rust binary is the **single source of truth for every number** — it is fully
deterministic and reproducible. When run as a Claude Code skill, the model is used **only**
for the genuinely intelligent parts: categorizing the long tail of unknown merchants,
assigning ambiguous transfer counterparties, and writing the recommendations narrative. The
model never computes totals.

---

## Requirements

- Rust (stable) with `cargo` — built/tested on 1.95.
- No network at runtime. (Building fetches crates once.)

---

## Install

### As a Claude Code skill (recommended)
Symlink the repo into your skills directory so Claude Code discovers it:

```bash
ln -s "$(pwd)" ~/.claude/skills/where-did-money-go
```

Then in Claude Code, just ask things like *"analyze my statements"* or *"where did my money
go last month"* and the skill triggers. The skill builds the binary on first use
(`scripts/build.sh` caches it and only rebuilds when sources change).

### As a standalone CLI
```bash
cd wdmg
cargo build --release
# binary at wdmg/target/release/wdmg
```
Everywhere below, `wdmg` means `./wdmg/target/release/wdmg` (or use `scripts/run.sh <args>`,
which builds-if-needed and forwards arguments).

---

## Quick start

```bash
# 1. Create a working/config directory seeded with the default configs.
wdmg init ~/.config/wdmg

# 2. Analyze a folder of CSV exports -> analysis.json + review_queue.json
wdmg analyze --input /path/to/exports --config ~/.config/wdmg --out ~/.config/wdmg

# 3. (Optional, via the skill) categorize unknown merchants + write insights.json

# 4. Render the report
wdmg report --analysis ~/.config/wdmg/analysis.json \
            --insights ~/.config/wdmg/insights.json \
            --out ~/.config/wdmg/report.html
# open report.html in any browser
```

`--insights` is optional; without it the report renders everything except the narrative
recommendations section.

> `analyze` reads the CSV files **directly in `--input`**, not nested subfolders. Point it at
> the specific folder you want, or run it per folder.

### One command, no LLM

To go straight from a folder of exports to a report with **no Claude / no LLM involved** at
all (pure deterministic pipeline: build → analyze → report), use the wrapper:

```bash
scripts/report.sh --input /path/to/exports
# -> writes /path/to/exports/wdmg-report/report.html (printed on the last line)
```

It builds the binary if needed, seeds an editable default config the first time, analyzes,
and renders. Optional flags: `--config <dir>` (your tuned `categories.toml`/`people.toml`;
defaults are written if absent) and `--out <dir>` (default `<input>/wdmg-report`). Edit the
config and re-run to refine. Everything in the report is complete except the LLM-authored
**Recommendations** narrative.

---

## Browser app (no regeneration needed)

`web/` is the same report as a **single offline HTML app** that loads data sets itself
(it is what the live demo serves). The Rust engine is compiled to WebAssembly and runs in
the page, so the numbers are identical to the CLI's (verified by a deep-equal smoke test).

- **⚙ Configuration** manages *projects* (statement CSVs + config TOMLs + optional
  `insights.json`), stored in the browser's IndexedDB. You can upload and download
  individual files, export or import a project as a `.zip`, and create, rename or delete
  projects. The bundled **demo** project can't be deleted; **↺ Reset demo** restores it.
  No file leaves the browser.
- **Map columns…** teaches the app a statement layout it doesn't recognise (pick the date,
  amount and description columns); the format is saved in the project's `profiles.toml`.
- **People & accounts** edits the household (`people.toml`) without writing TOML: who owns
  which account, the account's kind, and which statement files it "picks up" — set up by
  example with a guide that proposes a durable rule and previews exactly which files and rows
  it catches. A health check flags files on no account, idle accounts and broken rules.
- **Categories & rules** (⚙ Configuration, the report's "Sort them…" notice, or ✎ on any
  transaction) edits `categories.toml` and the transfer rules:
  - *To sort* lists uncategorized transactions; **Sort…** makes a rule from one. It shows
    every transaction the text would catch, where each is now and where it would end up
    (computed by the engine before you apply), with row details. Untick rows to give the
    text its own **amount range** (e.g. `SHELL` up to $20 → Snacks, from $35 → Gas), or
    choose **only the ones I tick** to pin one-offs (a car bought by bank cheque).
  - It spots near misses — an existing text whose range leaves the row out, or one that
    almost matches (`FISHY JOES` vs `FISHY JOE`) — and offers to edit it instead.
  - *Categories* shows every category and its texts with how many transactions each catches;
    click a text to edit it the same way, or rename, move, switch off and set kinds.
  - *Transfer rules* explains what counts as a transfer, shows what was paired automatically,
    placed by a rule or is still unknown, and lets you reorder, edit and add rules.
  - Nothing is saved until **Save**; edits keep the files' comments, and **? Help** explains
    matching, amount ranges and the pattern syntax.
- **Transactions** live in a floating window (move, resize, tile to a screen edge, minimize
  to the dock); clicking a category, merchant, chart bucket, flag or transfer lists its
  transactions there. Any section can be popped out into its own window (⇱).
- **Money between accounts & people** is a draggable circle of accounts and people with a
  ribbon per direction; click a name to drill into where its money came from and went.
- **AI insights:** export the project zip, ask Claude (this skill) to run the insight
  step on it (`data/` is the `--input`, `config/` the `--config`), then upload the
  resulting `insights.json` back into the project.
- **Cash flow → View: Change points** shows separate Net / In / Out charts. Grey dots are
  the buckets (Auto / Weekly / Bi-weekly / Monthly, as before) and the stepped line is the
  detected level with its 95% band. A **+N / −N** mark at each level change is green when
  it's good for your money and red when it isn't, and a red ring flags a bucket unusually
  high or low for its level (XmR limits). Life events and unusual-activity tags are
  overlaid; hover an unusual-activity mark on the top edge for what it flags. **Standard / Sensitive** set the detection penalty (PELT, robust noise
  estimate). "New level after N days" (default 14) is the minimum level duration; when
  the engine picks monthly blocks because paydays make weekly data too noisy, the
  effective minimum is ~2 months and the chart says so. Each level lists its unusual-bucket
  rate and, for Net, its share of net-negative weeks, both with Wilson 95% intervals.

```bash
scripts/build-wasm.sh          # Rust -> web/src/wasm (needs wasm32 target + wasm-bindgen-cli 0.2.108)
cd web && yarn install
yarn dev                       # or: yarn build  -> dist/index.html (one file, opens via file://)
yarn test                      # unit tests (storage, zip, rule and account-matching helpers)
node scripts/verify-e2e.mjs    # headless-Firefox end-to-end check of dist/index.html
yarn deploy                    # build and copy to ../docs/index.html (GitHub Pages)
```

## How configuration works

Three TOML files live in your config directory. `wdmg init` writes the defaults; edit them by
hand or with the subcommands below. An optional fourth file, `learned.toml`, holds rules the
LLM infers; configs are layered **your files > learned.toml > shipped defaults**.

### `profiles.toml` — how to detect & parse each export
Each `[[profile]]` is tried top-to-bottom; the first whose `match` rules all pass wins.

```toml
[[profile]]
id = "my_bank"
[profile.match]
header_first_cell = "Date"            # first header cell after BOM/quote strip ("A|B" = regex alt)
header_contains   = ["Amount", "Memo"] # all must appear in the header line
content_probe     = 'Category:'        # optional regex over the first data rows
filename          = "(?i)mybank"       # optional basename regex (disambiguate shared headers)
[profile.encoding]
has_bom = false
[profile.layout]
skip_blank_rows_after_header = 0
[profile.date]
column  = "Date"
format  = "%m/%d/%Y"                   # or formats = ["%m/%d/%Y", "%-m/%-d/%Y"] (first that parses)
[profile.amount]
column = "Amount"
sign   = "signed"                      # signed | outflow_positive | separate_debit_credit
[profile.merchant]
column      = "Name"
strip_codes = '^\[(?:CW|PR)\]\s*'      # optional leading code to remove
trim        = "trailing_whitespace"    # optional, for fixed-width padding
[profile.memo]
column           = "Memo"
category_harvest = 'Category:\s*(?P<cat>.+?)\s*$'  # optional bank-provided category hint
[profile.type]
column = "Transaction"
map    = { DEBIT = "spend", CREDIT = "income", PAYMENT = "transfer" }
[profile.person]
column = "Card Holder Name"            # when the export names the cardholder
[profile.account]
kind     = "debit"                     # debit | credit_card | savings | ...
currency = "CAD"
id_from_filename = '(\d{4})'           # capture e.g. a card's last-4 from the filename
```

**Tips.** Two sources with an identical header are disambiguated by `content_probe` if their
rows differ, otherwise by `filename`. `%-m`/`%-d` parse non-zero-padded month/day. Unmapped
`type` values fall through to category rules.

### `categories.toml` — how to label & sign transactions
```toml
[[category]]
name = "Car"
kind = "spend"                 # spend | income | transfer
  [[category.subcategory]]
  name = "Gas"
  patterns = ["SHELL", "ESSO"] # unanchored, case-insensitive regex over the normalized merchant
  amount_min = "20.00"         # optional; decimal as a string. only matches if |amount| in range
  # amount_max = "..."
  # priority = 10              # higher wins ties; default 0
  # A text with its OWN amount range (replaces the subcategory's; none = any amount):
  # ranged = [{ pattern = "SHELL C", amount_max = "20" }]

# One transaction put somewhere by hand, whatever the patterns say (date + signed amount +
# the bank's description identify it):
# [[pin]]
# date = "2999-03-14"
# amount = "-18450.00"
# description = "Official Cheque Ref 1"
# category = "Car"
# subcategory = "Purchase"
```

- **Most-specific match wins**: the longest matching pattern, then highest `priority`, then
  config order. (So a bare `UBER` loses to `UBER EATS`.) A `[[pin]]` beats every pattern.
- **`kind = "transfer"`** is the double-count fix — those transactions are netted out of both
  inflow and outflow and fed to transfer reconciliation. Use it for credit-card payments and
  movements between your own accounts/people. Give such rules a high `priority`.
- Unknown spend/income merchants (matched by nothing) land in `review_queue.json` for the LLM
  step to classify.

### `people.toml` — household & transfer ownership
```toml
[[person]]
id = "me"
display_name = "Me"
  [[person.account]]
  id = "my_card"
  cardholder = "ALICE"          # match by the profile's person column (case-insensitive)
  # card_last4 = "1234"         # match by the last-4 captured via id_from_filename
  # filename   = "(?i)debit"    # match by source basename regex
  # profile    = "mastercard_credit"  # restrict to one profile
  kind = "credit_card"

[[person]]
id = "partner"
display_name = "Partner"
  [[person.account]]
  id = "partner_savings"
  external = true               # no transactions of its own; a valid transfer destination
  kind = "savings"

[[transfer_rule]]
pattern  = "(?i)e-Transfer (To|From):?\\s*Partner"
to       = "partner"            # attribute the other leg of matching transfers here
priority = 10
```

An account matches a transaction by any combination of `cardholder` / `card_last4` /
`filename` / `profile` (all listed criteria must hold). `external = true` accounts receive
transfers but hold no transactions. Unmatched transfer counterparties surface in the review
queue.

### Editing config from the CLI (lossless TOML round-trip)
```bash
wdmg add-category   --config <dir> --name "Hobbies" --kind spend
wdmg add-rule       --config <dir> --category "Hobbies" --sub "Music" --pattern "MUSIC SHOP" [--min 25] [--max 500] [--priority 5]
wdmg add-person     --config <dir> --id partner --name "Partner"
wdmg assign-account --config <dir> --person partner --id partner_card --profile first_bank_card --kind debit [--cardholder NAME] [--card-last4 1234] [--filename '(?i)foo'] [--external]
wdmg assign-transfer --config <dir> --pattern '(?i)to:\s*partner' --to partner [--priority 8]
```
To remove something, edit the TOML directly (it round-trips cleanly), then re-run `analyze`.

---

## The report

A single `report.html` (opens offline) with: headline totals + savings rate, an interactive
cash-flow chart (legend toggles, All / 12mo / YTD range), spend-by-category, income-by-source,
a **"money between your accounts & people"** section (matched transfers, money sent to
external accounts, and a flow graph), recurring/subscriptions, anomalies, trends, per-person
and per-account breakdowns, recommendations (from `insights.json`), top merchants, and a
dedup/provenance footer. There is a light/dark toggle. No network is ever contacted.

**Slice and dice.** A sticky filter bar lets you narrow the whole report by **person,
account, category, kind**, a **month range**, and a merchant search — and *every*
visualization and table (the headline cards, cash-flow chart, category/income breakdowns,
recurring/anomalies/trends, the per-person/-account tables, top merchants, and the transfer
graph) recomputes live to respect the active filter. The Rust engine remains the single
source of truth: it embeds each transaction's exact decimal amount in the report, and the
browser re-aggregates in integer cents, so a filtered slice is as exact as the full total
(the unfiltered view reproduces the engine's figures to the cent).

---

## Privacy

The configs committed here are a **themed template**, not anyone's real data. Keep your real
configuration — which may contain actual names, employers, and cardholder strings — **outside
the repository**, e.g. in `~/.config/wdmg/`. Generated artifacts (`analysis.json`,
`review_queue.json`, `insights.json`, `learned.toml`, `report.html`) are produced from your
real statements and are `.gitignore`d; do not commit them.

---

## Development

```bash
cd wdmg
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test          # unit + integration + round-trip property tests
```

Layout: `wdmg/src/{model,config,ingest,normalize,dedup,categorize,identity,reconcile,
analytics,emit,report}` (+ `main.rs`); shipped configs and the vendored chart library in
`assets/`; the skill playbook in `SKILL.md`; deeper docs in `references/`.

## Credits / licenses

Bundles [µPlot](https://github.com/leeoniya/uPlot) (MIT) inlined into the report for offline
charting; its license banner is retained in the output.
