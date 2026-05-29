# Playground — a Futurama-themed demo dataset

A self-contained, **fictional** dataset that exercises every feature of the
report, so you can explore the tool without using real statements. No real names,
places, or account numbers — just Fry, Leela, Bender & co.

## Run it

```bash
./playground/generate.sh
# opens nothing; then open playground/report.html in a browser
```

`generate.sh` (re)builds the `wdmg` binary if needed, writes the CSV exports into
`playground/data/`, analyzes them with `playground/config/`, and renders
`playground/report.html` (fully offline).

> The generated `data/`, `analysis.json`, `review_queue.json`, and `report.html`
> are git-ignored — the **generator and config are the source of truth**; anyone
> can reproduce the rest with one command.

## The story (and the features it shows off)

Fry (`fry`) and Leela (`leela`) share a household budget across five accounts
(two credit cards, a joint chequing + savings ledger, and Leela's card). Bender
and assorted creatures are external counterparties.

| What happens | Feature it demonstrates |
|---|---|
| Planet Express pays salaries twice a month | recurring **income**, cash-flow chart |
| **Bender pays rent to Fry** in 2022–2023, then **Fry pays rent to Bender** in 2024–2025 (Fry moved out) | rent flips from **income → spend**; **trends** & recurring detection both ways |
| Fry inherits from the estate of Yancy Fry Sr (2024-06) | one-time income spike → **anomaly**, headline impact |
| Hypnotoad TV / Slurmstream / Bending Gym every month | **recurring / subscriptions** |
| Mars Vegas resort blowout (2023-12) and a jewellery splurge (2024-02) | **spend anomalies** |
| e-Transfers to Zoidberg, Amy, Hermes, Nibbler… | **E-transfers grouped by recipient** — no config needed |
| e-Transfers to Leela | household transfer attributed via a `transfer_rule` |
| Monthly credit-card payment; chequing → savings move | **matched transfer pairs** + the flow graph |
| RRSP / TFSA contributions | **external** transfer destinations |
| Groceries, restaurants, coffee, gas, utilities, telecom | **category → subcategory** breakdowns |
| An overlapping “recent” chequing export | cross-file **dedup** report |

Everything is deterministic (the generator is seeded), so re-running produces an
identical dataset. Tune `generate_data.py` to invent your own scenarios.
