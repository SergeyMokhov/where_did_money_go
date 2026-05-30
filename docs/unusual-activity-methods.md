# "Unusual activity" — analysis methods & persona map

Research notes for extending the `wdmg` *Unusual activity* section beyond the three
detectors shipped today. The guiding constraint is the product's spine: **offline,
deterministic, explainable**. Every method below must produce a human-readable
*reason* (what, vs. what baseline, how surprising), compute money in `Decimal`, and
fail closed on degenerate input. That rules black-box ML (isolation forest,
autoencoders) down to "optional, last" — they flag without explaining, and an
offline single-file report can't justify them to the user.

## What ships today

| Method | What it catches | Scope |
|---|---|---|
| `mad_zscore` | a single transaction far from its **subcategory** median (robust, `>3.5σ`) | per subcategory, ≥4 txns |
| `rare_large` | a big one-off in a subcategory too small for MAD (log-space global upper outlier) | rare subcategories |
| `monthly_iqr` | a whole month above a category's `Q3 + 1.5·IQR` | per category, ≥4 months |
| recurring (support) | stable cadence + low CoV → excluded from the above | per merchant |

These cover **point/amount outliers** well. The gaps are **duplicates, fraud
signals, subscription dynamics, seasonality, regime change, and compliance
patterns** — which is where the personas diverge.

## Method catalogue (grouped)

**Amount/point outliers** — *have.* MAD z-score, log-space global outlier,
IQR/Tukey fence. Cheap, robust, explainable.

**Duplicate / double-charge** — group by `(merchant, |amount|)`; two non-recurring
charges within ≤2 days = likely double-bill or accidental re-charge. Universally
useful, trivial, high signal.

**Subscription dynamics** (extends recurring detection):
- *Price increase / creep* — a recurring series with a **level shift** ("Netflix
  $16.99 → $20.99 on 2025-04"). Detect by splitting the amount series at its
  largest step and testing for two stable plateaus.
- *Free-trial → paid* — a new recurring series whose first charge follows a $0 or
  absent period.
- *Duplicate / overlapping subs* — two recurring merchants in the same category
  (two music services, an old + new plan running together).
- *Zombie subs* — recurring charge with no correlated activity (best-effort; we
  can't see usage, only co-occurring spend).

**Fraud signals** (card-level):
- *First-seen merchant* — a charge from a merchant never seen before; escalate when
  large or foreign. (Strong consumer fraud cue.)
- *Card-testing velocity* — a **burst** of tiny charges (≪ typical) from new
  merchants within a short window — the classic carding fingerprint.
- *Geographic / FX* — foreign-currency or foreign-merchant charges and FX markup;
  `wdmg` already records `currency` per profile.

**Time-series structure:**
- *STL / SH-ESD* — decompose a category's (or income's) series into
  trend+seasonal+residual and flag **residual** outliers. Beats `monthly_iqr` for
  seasonal lines (heating, holidays, quarterly bills): a high December isn't an
  anomaly if every December is high. SH-ESD uses the **median + MAD** of the
  remainder so the anomaly doesn't corrupt its own baseline.
- *Change-point (CUSUM / PELT)* — a **sustained** shift in level: income dropped
  permanently, a new recurring cost appeared, lifestyle inflation after a raise.
  Distinct from a one-month spike, and a natural feeder for the life-events markers
  ("something changed here on 2025-07").

**Forensic / compliance:**
- *Benford's law (first-digit)* — fabricated/round expense and invoice amounts
  skew the natural `1`→30.1% … `9`→4.6% leading-digit curve. A dataset-level test
  (chi-square or MAD vs Benford) over expense/vendor amounts; needs ≥~100–300
  values to be meaningful. Used in real forensic audits (WorldCom, municipal vendor
  payments).
- *Structuring / near-threshold* — cash or transfers clustering **just below** a
  reporting threshold (Canada: FINTRAC large-cash-transaction report at **CAD
  $10,000**; US CTR at **USD $10,000**). Flag magnitudes in `[0.9·T, T)`, escalate
  on ≥3 within 30 days or split across accounts.
- *Round-number to individuals* — many exactly-round transfers/e-transfers to
  people (informal payments, gifts) — optics for public figures.
- *New / unexpected counterparty* — a large inflow from a never-seen source, or
  concentration of payments from one entity (undisclosed income, influence).

**Black-box ML** (isolation forest, LOF, autoencoders) — multivariate, powerful,
but opaque, non-deterministic by default, and at odds with an offline explainable
report. **Defer / optional only.**

## Persona map

The same data, different fears. "✦" = top priority for that persona.

| Method | Avg consumer | IT prosumer | Business owner | Public figure |
|---|:--:|:--:|:--:|:--:|
| Duplicate charge | ✦ | ✓ | ✦ (dup invoice) | ✓ |
| First-seen merchant (large/foreign) | ✦ | ✓ | ✓ | ✓ |
| Card-testing velocity | ✦ | ✓ | ✓ | ✓ |
| Subscription price-increase / creep | ✓ | ✦ | ✓ | · |
| Duplicate / overlapping subscriptions | ✓ | ✦ | ✓ | · |
| Free-trial → paid | ✓ | ✦ | ✓ | · |
| FX / foreign charge | ✓ | ✦ | ✓ | ✓ |
| STL / seasonal residual | · | ✓ | ✦ | ✓ |
| Change-point (regime shift) | ✓ | ✦ | ✦ | ✓ |
| Benford's law (expense/invoice) | · | · | ✦ | ✦ |
| Structuring / near-threshold cash | · | · | ✓ | ✦ |
| Round-number to individuals | · | · | ✓ | ✦ |
| New / unexpected income source | ✓ | ✓ | ✓ | ✦ |
| Amount outlier (MAD / rare_large) — *have* | ✓ | ✓ | ✓ | ✓ |

- **Average consumer:** safety + stop waste. Duplicates, unfamiliar-merchant fraud,
  card-testing, forgotten/creeping subscriptions.
- **IT prosumer:** SaaS-heavy, USD charges, data-literate. Subscription price hikes,
  overlapping tools, free-trial conversions, FX markup, lifestyle-inflation
  change-points.
- **Business owner:** cash flow + expense integrity + seasonality. Benford on
  expenses, duplicate invoices, new-vendor large payments, seasonal-aware (STL)
  anomalies, revenue regime shifts.
- **Public figure:** scrutiny + optics + disclosure. Near-threshold/structuring,
  unusual cash, round-number transfers, new/undisclosed income sources, sudden
  net-worth jumps, Benford on declared expenses.

## Design principles for any new detector

1. **Explain or don't ship it.** Each flag carries method, baseline, severity, and a
   plain-language reason. (Why ML is last.)
2. **Robust + scoped + fail-closed.** Median/MAD over mean/σ; scope pools to
   like-for-like; exclude known recurring; log-transform skewed magnitudes; absolute
   floors so "large" means large; on degenerate spread (MAD=0, too few points)
   produce **no** false anomaly.
3. **Normalize severity** across methods so the combined list ranks sensibly.
4. **Persona lens (live, nested).** The engine **always computes every detector**;
   the report exposes a `consumer | prosumer | business | public` lens that *filters
   the precomputed list* client-side, so a reader can switch personas without
   re-running. The lens is a strict **nesting** — each level adds checks on top of
   the simpler one:
   - **consumer** — fraud + waste + amount outliers (the actionable everyday set).
   - **prosumer** — *+ time-series* (`seasonal_residual`, `change_point`).
   - **business** — *+ expense/cash forensics* (`benford`, `structuring`).
   - **public** — *+ round-number payments to individuals* (`round_number`; optics).

   `settings.toml`'s `analysis_profile` only picks the **default** lens the report
   opens with; jurisdiction params (structuring threshold/currency) still come from
   settings. Defaults stay conservative (consumer).
5. **Deterministic.** Same inputs → byte-identical `analysis.json`. Implement in the
   Rust engine **and** mirror in the report's client-side recompute (they must
   agree, as `rare_large` now does).

## Suggested build order (value ÷ effort)

- **Tier 1 — everyone, cheap:** duplicate-charge · first-seen-merchant (large/
  foreign) · subscription price-increase · FX/foreign flag.
- **Tier 2 — targeted:** card-testing velocity · structuring/near-threshold ·
  round-number-to-individual · Benford (business/public).
- **Tier 3 — heavier, high payoff:** STL/SH-ESD seasonal residual · CUSUM/PELT
  change-point (also auto-suggests life-event markers) · cash-flow runway.
- **Tier 4 — optional:** isolation forest / autoencoder, only behind an explicit,
  clearly-labelled "experimental, unexplained" toggle.

## Sources

- Subscription creep / hidden recurring charges — Era, bankstatement.app
- Time-series methods (STL, SH-ESD, CUSUM, isolation forest, change-point) — ML
  Journey; detectoutliers.com; arXiv "Time-Series Anomaly Detection: A Decade Review"
- Structuring / smurfing & the $10,000 threshold — ComplyAdvantage; Sanction
  Scanner; Wikipedia "Structuring"
- Card-testing / velocity / first-merchant / geographic & CNP rules — Chargeflow;
  Spec; ByteByteGo "How Stripe detects fraud"
- Benford's law for expense/invoice fraud — ACFE; MetricGate; Wikipedia "Benford's
  law"
