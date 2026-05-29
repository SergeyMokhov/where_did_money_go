---
name: where-did-money-go
description: >-
  Analyze bank/credit/debit card statement exports and produce an interactive,
  fully-offline HTML report of where money came from and went. USE THIS whenever the
  user asks to analyze statements or transactions, categorize spending, see a spending
  report or budget breakdown, find subscriptions/recurring charges, detect spending
  anomalies, understand income vs spending, track transfers between their accounts or
  people, or asks "where did my money go". Reads CSV exports from multiple banks, cards,
  and people (handles overlapping files and dedups them). Categories, bank profiles, and
  household identity are configurable TOML; the skill can interactively add/remove
  categories, people, and transfer assignments. All heavy work runs in a Rust binary
  (wdmg); the LLM is used only to categorize unknown merchants, assign ambiguous
  transfers, and write the recommendations narrative.
version: 0.1.0
---

# where-did-money-go

A statement analyzer. A **Rust binary (`wdmg`)** does all deterministic work — parse,
normalize, dedup, categorize, reconcile transfers, compute every statistic — and is the
single source of truth for the numbers. **You (the LLM) only** categorize the long tail of
unknown merchants, assign ambiguous transfer counterparties, and author the
recommendations/insights prose. Never compute totals yourself.

## Files & layout
- `wdmg/` — the Rust crate. Build/cache via `scripts/build.sh` (prints the binary path).
  Always invoke the binary through `scripts/run.sh <subcommand> ...`.
- `assets/default_{profiles,categories,people}.toml` — shipped defaults copied into the
  working config dir by `wdmg init`.
- Working dir (default `~/.config/wdmg` or a user-given dir) holds the user's
  `profiles.toml`, `categories.toml`, `people.toml`, the LLM overlay `learned.toml`, and
  the generated `analysis.json`, `review_queue.json`, `insights.json`, `report.html`.

## Config model (read `references/profiles-and-config.md` before editing configs)
- **profiles.toml** — how to detect & parse each bank's CSV (header signature, dates,
  amount sign, column mapping, person/account attribution). Add a bank = add a `[[profile]]`.
- **categories.toml** — `[[category]]` (name + `kind` = spend|income|transfer) with
  `[[category.subcategory]]` patterns (unanchored case-insensitive regex) + optional
  amount bounds. `kind=transfer` is netted out of both flows (no double counting).
- **people.toml** — household: people own cards/accounts (by cardholder / card last-4 /
  filename); external accounts (savings) are valid transfer destinations; `transfer_rule`
  entries assign a transfer's counterparty to a person/account.
- **learned.toml** — your inferred rules (categories + transfer assignments), layered as
  **user > learned > defaults**. Carry `source = "llm"`, `confidence`, `learned_on`.

## Workflow

1. **Locate input.** Ask the user for the directory of CSV statement exports (there is no
   built-in default — the user points you at their export folder). Determine/confirm the
   working config dir (default `~/.config/wdmg`; create with `wdmg init <dir>` if it has no
   `*.toml` yet).

2. **Analyze.** `scripts/run.sh analyze --input <export-dir> --config <cfg-dir> --out <cfg-dir>`.
   This writes `analysis.json` (all stats, transfer graph, anomalies, trends, dedup report)
   and `review_queue.json` (unknown/low-confidence merchants + unassigned transfer
   counterparties, each with sample descriptions, amounts, counts, date range, owner).

3. **Resolve the review queue (the only real LLM step).** Read `review_queue.json`.
   - For each unknown **merchant**, decide its category/subcategory and a *generalized*
     pattern (e.g. `SHELL C81432` → `Car/Gas`, pattern `SHELL`).
   - For each unassigned **transfer counterparty**, decide the target person/account.
   - **Auto-apply** high-confidence decisions by writing rules to `learned.toml` (via
     `add-rule` / `assign-transfer`, or by editing `learned.toml` directly).
   - **Confirm only low-confidence** ones with the user (use AskUserQuestion, batch them).
     Do not pester about obvious matches.
   - Re-run `analyze`. The queue should shrink; numbers refresh deterministically.

4. **Author insights.** Read `analysis.json`. Write `insights.json` with: a short narrative
   of where money came from / went, concrete **"ways to save"** recommendations grounded in
   the data (cite categories, recurring charges, anomalies, trends), plain-language
   explanations of flagged anomalies, and notable trends. Numbers come from analysis.json —
   quote them, don't recompute. See `references/analytics.md` for what each field means.

5. **Report.** `scripts/run.sh report --analysis <cfg>/analysis.json --insights
   <cfg>/insights.json --out <cfg>/report.html`. Tell the user the path; it opens directly
   in a browser (no server). Offer to open it.

## Interactive management
- Add/remove category or rule: `wdmg add-category --name X --kind spend`,
  `wdmg add-rule --category X --sub Y --pattern Z [--min .. --max ..]`. To remove, edit the
  TOML (it round-trips losslessly). Re-run `analyze`.
- Household: `wdmg add-person --id .. --name ..`,
  `wdmg assign-account --person .. --profile .. --match <last4|cardholder|filename-regex>`,
  `wdmg assign-transfer --pattern <re> --to <person|account>`.

## Rules
- The LLM never produces numbers — only category/transfer rules and prose.
- Confirm-low-confidence only; auto-apply the confident majority.
- The report must stay fully offline (uPlot is inlined); never add CDN/network references.
- Money is decimal; never reason about cents as floats.
