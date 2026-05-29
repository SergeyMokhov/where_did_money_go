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

Set in the year **2999**. **Fry** (`fry`), **Leela** (`leela`) and **Bender**
(`bender`) each have their own income and accounts; Professor Farnsworth signs the
Planet Express payroll. Fry & Leela share a joint chequing + savings; Bender keeps
his own ledger. Zoidberg, the casinos and the fembots are external counterparties.

| What happens | Feature it demonstrates |
|---|---|
| Farnsworth pays Fry, Leela & Bender twice a month; Bender also earns a 2nd salary bending girders at IRONWORKS | recurring **income**, **by-person/-account**, cash-flow chart |
| **Bender pays rent to Fry** in 2997–2998, then **Fry pays rent to Bender** in 2999 (Fry moves to the Robot Arms Apts) | rent flips from **income → spend**; **trends** both ways |
| Bender gambles, drinks and e-Transfers his way into **bankruptcy** (see the puzzles) | **negative net**, spend-vs-transfer distinction, trends |
| Fry inherits **$50k** from the Estate of Yancy Fry Sr (2999-06) | one-time income spike → **anomaly** |
| Hypnotoad TV / Slurmstream / EyePhone / shuttle fuel every month | **recurring / subscriptions** |
| Mars Vegas resort blowout (2998-08) and a jewellery splurge (2999-02) | **spend anomalies** |
| e-Transfers to Zoidberg / Leela / fembots | **E-transfers grouped by recipient**; one `transfer_rule` attributes Leela |
| Monthly credit-card payment; chequing → savings move | **matched transfer pairs** + the flow graph |
| RRSP / TFSA contributions | **external** transfer destinations |
| Leela's pet **Nibbler** (vet, chow); Fry's gas-station sandwiches | **category → subcategory** breakdowns |
| An overlapping “recent” chequing export | cross-file **dedup** report |

There are a few **hidden puzzles** planted in the data — see
[`spoilers.md`](./spoilers.md) once you've tried to solve them yourself.

The data under `data/` is committed (generate once, use forever). It's
deterministic (the generator is seeded `random.seed(2999)`), so re-running
`generate_data.py` reproduces it exactly. Tune the generator to invent your own.
