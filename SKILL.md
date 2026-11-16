---
name: where-did-money-go
description: >-
  AI insights for the where-did-money-go personal-finance browser app. USE THIS when the
  user gives you a file exported from that app — an "ai-summary.json" (aggregated figures)
  or a project .zip (data/*.csv + config/*.toml) — and asks for insights, a summary,
  recommendations or ways to save; or when they ask you to suggest category rules for
  their uncategorized transactions. You write insights.json (and optionally learned.toml)
  for the user to upload back into the app. The numbers come from the app's Rust engine
  (wdmg); you never compute totals yourself.
version: 0.2.0
---

# where-did-money-go — AI insights

The product is an **offline browser app** (`web/`, live at
https://sergeymokhov.github.io/where_did_money_go/): it reads bank/card CSV exports, analyzes
them in the page with the Rust engine compiled to WebAssembly, and never makes a network call.
Everything except the AI step works there without you.

**Your job is the optional AI step:** the user exports a file from the app
(⚙ Configuration → ✨ AI insights), gives it to you, and uploads what you write back into the
app. You only write prose and rules — **every number comes from the engine**; quote them,
never recompute.

## Inputs (one of)

- **`*-ai-summary.json`** (`"kind": "wdmg-ai-summary"`) — aggregates only: period (months),
  totals, `monthly`, `spend_by_category`, `income_by_source`, `top_merchants`, `recurring`,
  `unusual_activity`, `trends`, `by_person` (pseudonymous "Person N"). No transactions, bank
  wording, account names or exact dates — don't ask for them. Go straight to *Write insights*.
- **A project `.zip`** — `data/*.csv` (statements), `config/*.toml`, maybe `insights.json`.
  Unzip it to a scratch dir and run the engine:
  `scripts/run.sh analyze --input <dir>/data --config <dir>/config --out <dir>`
  (builds the binary on first use). That writes `analysis.json` (all figures; see
  `references/analytics.md`) and `review_queue.json` (uncategorized merchants, unassigned
  transfers).

## Write insights → `insights.json`

Read the summary / `analysis.json` and write:

```json
{
  "summary": "2–4 sentences: where the money came from and went, the net, anything striking.",
  "recommendations": [
    { "title": "Short, concrete action", "detail": "Why, citing the figures (category, merchant, recurring charge, flag, trend).", "est_monthly_savings": "120.00" }
  ],
  "trend_notes": ["One line per notable trend."],
  "anomaly_notes": [{ "ref": "merchant or category", "note": "plain-language reading of a flag" }]
}
```

- Ground every claim in the data; quote amounts as they appear (decimal strings).
- `est_monthly_savings` is optional — a defensible estimate from the figures, as a decimal string.
- Be specific (name the subscription, the category, the month) and kind; flags aren't accusations.
- Save it as `insights.json` next to the input and tell the user to upload it:
  ⚙ Configuration → ✨ AI insights → ⇪ Upload insights.json.

## Optional (project .zip only): suggest category rules → `learned.toml`

When `review_queue.json` lists uncategorized merchants or unassigned transfers:
- Decide a category/subcategory and a **generalized** pattern for each merchant
  (`SHELL C81432` → `Car › Gas`, pattern `SHELL`); for transfers, the person/account on the
  other side.
- Write the confident category rules to `learned.toml` (same format as `categories.toml`; see
  `references/profiles-and-config.md`), re-run `analyze` with it in `config/` to check the queue
  shrinks, and **ask the user** (batched) about the uncertain ones.
- `learned.toml` holds categories only. For transfers, list your suggestions ("e-Transfers to
  JANE → Jane") for the user to add in the app: Categories & rules → Transfer rules.
- Tell the user to upload `learned.toml` into the project (⚙ Configuration → drop it in). It is
  layered under their own `categories.toml`, which the app's Categories & rules editor edits.

## Rules
- Never produce numbers the engine didn't — only prose and rules.
- Don't ask for more data than the user chose to share.
- Don't write anything into the app's files other than `insights.json` / `learned.toml`.
- Money is decimal; never reason about cents as floats.
