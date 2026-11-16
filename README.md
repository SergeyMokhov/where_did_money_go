# where-did-money-go

**An offline browser app that shows where your money came from and where it went** — from the
CSV exports of your bank accounts and cards, for one person or a whole household. It runs
entirely in your browser: once the page has loaded it makes **no network calls at all**, and
the browser itself enforces that.

**▶ [Open the app](https://sergeymokhov.github.io/where_did_money_go/)** — it starts on a demo
household. No install, no sign-up, no server. The demo (the *Futurama* crew, year 2999) keeps a
few secrets buried in the numbers — slice by person, search the merchants, and follow the ⓘ
hints to see if you can work out *where Bender's money really went* before the report spells it
out.

---

## What it does

- **Reads your bank's CSV exports** — most US, Canadian, UK and EU bank and card exports
  and accounting-import layouts (Xero, QuickBooks) without any setup. It works out the
  columns from the file: date, description, a signed amount or separate money-out/money-in
  columns, summary lines above the header, files with no header row, day-first or
  month-first dates, decimal commas, cards that write charges as positive numbers, and
  pending rows. If it can't make sense of a file — or guesses wrong — **Map columns…**: point
  at the date, amount and description, and it remembers the layout.
- **De-duplicates** overlapping downloads (a full-history file plus a recent one) so nothing is
  counted twice, while keeping genuine same-day repeats.
- **Sorts every transaction into categories** with built-in rules you can change, and treats
  money moving between your own accounts — paying off a card, chequing → savings — as a
  **transfer**, not as spending or income (the classic "my card payment looks like spending"
  double count).
- **Models a household:** people, their accounts and cards, and accounts you don't have
  statements for. It pairs the two halves of a transfer and draws who moved money to whom.
- **Answers the questions:** cash flow over time with detected changes in your levels of
  spending and income, where it went (category → subcategory → merchant), where it came from,
  recurring charges and subscriptions, unusual activity (outliers, duplicate charges, price
  hikes, …), trends, per person and per account, and top merchants.
- **Everything is filterable** — by person, account, category, kind, month range or merchant —
  and every chart and table follows. Click anything to list its transactions.

## Use it with your own statements

1. **Export CSVs from your bank.** On each account's or card's transaction page look for
   "Download" or "Export" → CSV, and take as much history as it offers.
2. **⚙ Configuration → + New project**, then drop the files in. Anything in an unknown layout
   gets a **Map columns…** button.
3. **👥 People & accounts:** say whose each account is. A guide groups your files into accounts
   by example and shows exactly which files and rows each one picks up.
4. **🏷 Categories & rules:** sort what the built-in categories don't catch. Pick a
   transaction, choose the text that identifies it and where it belongs, and see every
   transaction the rule would move before applying it. Rules can have their own amount range
   (`SHELL` up to $20 → snacks, from $35 → gas), and one-offs can be pinned individually.
5. **Back up:** ⇩ Export (.zip) — the project lives only in your browser.

A **Getting started** checklist in each new project walks through these steps.

## Privacy and your data

- **Nothing leaves the browser.** The page carries a strict Content-Security-Policy
  (`connect-src 'none'`, no external scripts, styles, images or fonts): the browser blocks any
  attempt to reach a server. You can check it in the page source and in your browser's
  developer tools — the network tab stays empty.
- **Where data lives:** each project (statements, configuration, optional insights) is kept in
  your browser's storage for this site (IndexedDB), on this computer only. Clearing the site's
  data deletes it — keep backups (⇩ Export). The app reminds you when a project has no recent
  backup.
- **Who could read it:** other websites can't. Other pages served from the same address,
  browser extensions with access to all sites, and anyone with access to your computer account
  could. For that, **protect a project with a passphrase**: it is then stored only encrypted
  (AES-GCM-256, key from PBKDF2-SHA256 with 600,000 iterations). Once unlocked, the key stays in
  that browser tab until you press **Lock now** or close the tab — lock it when you're done. A
  forgotten passphrase can't be recovered.
- The app's **🔒 Offline & private** button explains all of this in the app.

## AI insights (optional)

An AI can read your figures and write a short summary and concrete ways to save, shown under
*Recommendations*. This is the only step where data goes anywhere — and only because you hand
a file over yourself:

1. ⚙ Configuration → **✨ AI insights** → download a **summary for AI** (totals by month,
   category and merchant, recurring charges and flags — no transactions, bank wording, account
   names or exact dates) or the whole project.
2. Give it to [Claude Code](https://claude.com/claude-code) with this repo installed as a skill
   (`ln -s "$(pwd)" ~/.claude/skills/where-did-money-go`) and ask it to *write insights for this
   file*. With a whole project it can also suggest category rules (`learned.toml`).
3. Upload the `insights.json` it writes back into the project.

The skill's playbook is [`SKILL.md`](SKILL.md).

---

## Under the hood

The analysis engine is a Rust crate (`wdmg/`): parse → de-duplicate → identify accounts →
categorize → reconcile transfers → analytics. It is deterministic and uses exact decimal
money throughout. `wdmg-wasm/` compiles it to WebAssembly for the app, so the browser computes
exactly what the CLI computes (verified by a deep-equal test). The app (`web/`) is a single
HTML file — engine, code and demo data inlined — built with Vite.

```text
wdmg/        Rust engine + CLI            web/         the browser app (Vite + TypeScript)
wdmg-wasm/   engine → WebAssembly         playground/  demo data generator + config
assets/      built-in formats/categories  references/  config format, analytics notes
docs/        the deployed app (GitHub Pages)
```

### Build and test

```bash
scripts/build-wasm.sh          # Rust -> web/src/wasm (wasm32 target + wasm-bindgen-cli 0.2.108)
cd web && yarn install
yarn dev                       # dev server; or: yarn build -> dist/index.html (one file)
yarn test                      # unit tests (storage & encryption, zip, rules, AI summary, …)
node scripts/verify-e2e.mjs    # headless-Firefox end-to-end check of dist/index.html,
                               # incl. that the page can't reach the network
yarn deploy                    # build and copy to ../docs/index.html (GitHub Pages)

cd ../wdmg
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

Needs Rust (stable, built on 1.95), Node, and Firefox for the end-to-end test.

### Command line (advanced)

The same engine runs as a CLI — used by the AI-insights skill, and handy for scripting:

```bash
cd wdmg && cargo build --release     # binary: wdmg/target/release/wdmg (or scripts/run.sh <args>)
wdmg init ~/wdmg-config              # write the default config files
wdmg analyze --input <csv-dir> --config ~/wdmg-config --out <out-dir>
                                     # -> analysis.json + review_queue.json
wdmg add-rule --config <dir> --category Hobbies --sub Music --pattern "MUSIC SHOP"
                                     # also add-category, add-person, assign-account, assign-transfer
```

The configuration format (`profiles.toml`, `categories.toml`, `people.toml`) is documented in
[`references/profiles-and-config.md`](references/profiles-and-config.md); the figures in
`analysis.json` in [`references/analytics.md`](references/analytics.md).

The shipped example configuration is themed (Planet Express, New New York, …) — a template.
Keep your real configuration and statements outside the repository; generated files
(`analysis.json`, `review_queue.json`, `insights.json`, `learned.toml`) are git-ignored.

## Credits / licenses

Charts by [µPlot](https://github.com/leeoniya/uPlot) (MIT), bundled into the app.
