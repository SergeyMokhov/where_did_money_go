# Playground spoilers 🔮

> ⚠️ **Spoilers below.** The playground (`playground/data/`, year 2999) hides a few
> puzzles so exploring the report is a game, not a chore. Try them yourself in
> `report.html` first; the answers and the exact clicks are here.

Open the report with `./playground/generate.sh` (or `scripts/report.sh --input
playground/data --config playground/config`) and use the **filter bar** at the top
plus the **ⓘ** help icons on each section.

---

## 🧩 Puzzle 1 — "Figure out why Bender went broke."

**Answer:** Bender out-earns almost everyone — **two salaries** (Planet Express +
the IRONWORKS metalworking factory, ~$158k in over three years) — yet he ends up
roughly **-$210,000 net**. The money evaporates into, in order:

1. **Gambling** — ~**$317,000** at the casinos (blackjack, slots). This alone is
   bigger than his entire income. It escalates every year and peaks with a single
   **$8,000** night at LUNAR VEGAS CASINO in Sept 2999.
2. **e-Transfers to fembots** — ANGELYNE, BIG BERTHA, FEMBOT FANTASIES, LUSTRON
   ESCORTS. These are *transfers* (not "spend"), so they hide from the spend
   charts — you find them in the **E-transfers** section / transfer graph.
3. **Booze & cigars** — LOBRAU, DRINKS DEPOT, EL CHEAPO CIGARS.
4. **A credit card he never pays off** — his Mastercard (cardholder BENDER) racks
   up booze, cigars and casino *cash advances* with **no `PAYMENT - THANK YOU`
   ever** (unlike Fry's card, which is paid monthly). The balance just climbs.

To twist the knife, his **factory job ends in July 2999** (the IRONWORKS payroll
stops), so income drops just as the gambling peaks.

**How to find it in the report**
- Filter **Person = bender**. The headline **Net** card is deep red/negative and
  the **cash-flow** line crashes through zero — especially in 2999.
- **Where it went** → *Indulgence → Lottery & Gaming* dwarfs everything.
- **Top merchants** → the casinos sit at the top.
- **E-transfers** → the fembot recipients, growing year over year. (Toggle
  **Person = bender** and read who he paid.)
- **By account** → *Bender Mastercard* shows only purchases and **no card
  payment** (compare with *Fry Mastercard*, which is paid off monthly); the
  IRONWORKS salary on *Bender Ledger* disappears mid-2999.
- **Trends** → gambling trending sharply up.

---

## 🧩 Puzzle 2 — "Who spends the most on gas-station sandwiches?"

**Answer:** **Fry**, by a mile — about **$3,435** across **360** trips to the
GAS-N-GULP SANDWICH CO counter. Leela and Bender grab one occasionally (~$50 each),
but this is Fry's signature vice.

**How to find it**
- Type **`GAS-N-GULP`** (or `SANDWICH`) into the **merchant search** in the filter
  bar, then look at **By person** — Fry towers over the rest.
- Or filter **Person = fry** → *Takeout & Restaurants → Gas-Station Snacks*.

---

## 🧩 Puzzle 3 — "How much does it cost to fill up the Planet Express shuttle?"

**Answer:** **$1,200.00 per fill-up.** Leela (the captain) fuels the delivery
shuttle with **DARK MATTER FUEL DEPOT** once a month — **36 fills, $43,200 total**
over three years.

**How to find it**
- **Recurring & subscriptions** lists `DARK MATTER FUEL DEPOT` with a ~monthly
  cadence and a typical amount of **$1,200**.
- Or filter **Category = Car → Starship Fuel** (kept separate from ordinary Gas),
  or search the merchant `DARK MATTER`.

---

## 🥚 Bonus easter eggs

- **Fry keeps bailing out Dr. Zoidberg.** Monthly `INTERAC E-TRANSFER TO: ZOIDBERG`
  — small, faithful, never repaid. See the **E-transfers** section.
- **The rent flips.** Bender pays Fry rent in **2997–2998** (income), then Fry
  moves into the Robot Arms Apts and **pays Bender** in **2999** (spend). Watch the
  rent line invert in **Trends** / **Where it came from** vs **Where it went**.
- **Inheritance spike.** A one-time **$50,000** cheque from the *Estate of Yancy
  Fry Sr* lands June 2999 — a green spike on the cash-flow chart and an income
  anomaly.
- **Two splurges.** A **$5,500** MARS VEGAS RESORT trip (Aug 2998) and a **$3,000**
  WONG INDUSTRIES JEWELLERS gift for Leela (Valentine's, 2999) show up under
  **Unusual activity**.
- **Nibbler the pet.** Leela spends on INTERSPECIES VET visits and NIBBLONIAN CHOW
  — see *Pets → Pet Supplies & Vet* under **Person = leela**.
- **Cross-file dedup.** `joint_chequing_recent.csv` overlaps the last two months of
  `joint_chequing.csv`; the **dedup footer** reports the rows collapsed.
- **Unusual activity battery + the persona Lens.** The playground opens at the
  **Public** lens (`config/settings.toml` `analysis_profile = "public"`), the widest.
  The **Lens** dropdown *inside the Unusual activity section* switches persona live
  (no config edit) — it's a **nested** filter, each step adding checks:
  **Consumer** (fraud + waste + amount outliers) → **Prosumer** (+ time-series:
  season-adjusted spikes, regime shift) → **Business** (+ Benford) → **Public**
  (+ round-number payments to individuals). The note next to it tells you how many
  flags each higher lens would add. In this dataset look for: the two **rare big
  purchases** (Mars Vegas, the jewellery gift) and a **deficit streak** (Bender's
  2998 run) — visible to everyone; a **spending shift** (the 2998 rent flip) and
  **season-adjusted** spikes — Prosumer+; and a **Benford** flag (the synthetic
  amounts aren't natural) — Business+. (This synthetic data has no near-threshold
  *structuring* or irregular *round-number* clusters, so Business and Public happen
  to coincide here — on real data Public adds the round-number-to-individual flags.)
- **Life-event markers.** `config/events.toml` plants four amber markers on the
  cash-flow chart (crew hired, Mars Vegas getaway, Fry's inheritance, Bender's
  factory job ending). Watch how Bender's marker sits right where his income drops
  and the gambling peaks. Add your own (purple) markers from the report's
  **Life events** control under the chart — they save in your browser.

---

## Who earns what (ground truth)

| Person | Income source(s) | Habits / where it goes |
|---|---|---|
| **Fry** | Planet Express payroll (Prof. Farnsworth) | gas-station sandwiches, coffee, pizza, comics & arcade; bails out Zoidberg |
| **Leela** | Planet Express payroll (captain) | Nibbler (vet, chow), kendo dojo, groceries; fuels the shuttle; saves steadily |
| **Bender** | Planet Express payroll **+** IRONWORKS factory | blackjack, fembots, booze, cigars → bankruptcy |
| **Prof. Farnsworth** | (employer) | signs everyone's Planet Express payroll |

Everything is deterministic (seeded `random.seed(2999)`), so regenerating with
`python3 playground/generate_data.py` reproduces these exact numbers.
