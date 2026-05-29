# Analytics reference (what each `analysis.json` field means)

All amounts are exact decimals serialized as strings. The host agent reads these to write
`insights.json`; it must quote, not recompute, them.

## Flows
- `total_inflow`, `total_outflow`, `net` — sums of income and spend respectively, with
  `kind=transfer` rows excluded entirely (so credit-card payments and inter-account moves
  never inflate either side).
- `savings_rate` = `(income - spend) / income` over the period.
- `monthly[]`, `weekly[]` — per-bucket `{ period, inflow, outflow, net }` series (calendar
  month and ISO week). Each carries a 3-month / 4-week rolling mean baseline.

## By category / source
- `spend_by_category[]` — `{ category, subcategory, total, count, pct_of_spend,
  monthly[] }`. `income_by_source[]` — same shape for income-kind subcategories.
- `top_merchants[]` — normalized merchant, total, count, avg ticket.

## Recurring / subscriptions (`recurring[]`)
A merchant group flagged when: ≥3 occurrences, inter-transaction intervals cluster near a
period in {7, 14, 30/31, 365} days (median interval within tolerance), and the amount is
stable (coefficient of variation < ~0.15). Fields: `merchant`, `cadence`, `typical_amount`,
`occurrences`, `monthly_equivalent`, `predicted_next_date`. Use for "subscriptions you may
have forgotten" recommendations.

## Anomalies (`anomalies[]`)
Two complementary methods, each item tagged with `method`:
- `mad_zscore` — a single transaction whose amount is a robust outlier within its category:
  `|x - median| / (1.4826 * MAD) > 3.5`. Robust to the very outliers being detected.
- `monthly_iqr` — a whole month whose category spend exceeds `Q3 + 1.5*IQR` of that
  category's monthly history (or > 2× rolling stdev from the rolling mean). Distinguishes a
  single large purchase from a broadly expensive month.
Fields include `category`, `period_or_date`, `amount`, `baseline`, `severity`.

## Trends (`trends[]`)
Per category: month-over-month and year-over-year deltas (absolute decimal + %), plus a
`direction` (rising|falling|flat) from the sign of a least-squares slope over recent months.

## Transfer graph (`transfers`)
- `matched_pairs[]` — internal moves where both legs were found: `{ from_account,
  to_account, amount, out_date, in_date, confidence }`.
- `external[]` — a transfer leg assigned to an external/other account: `{ from_account,
  to (person/account), amount, date }`.
- `unresolved[]` — transfer legs not yet paired or assigned (these also appear in the
  review queue).
- `graph` — `{ nodes: [people/accounts], edges: [{from, to, net_amount, count}] }` for the
  report's flow visual.

## Breakdowns
- `by_person[]`, `by_account[]` — spend / income / net grouped by resolved owner and
  account, so "who spent what on which card" is answerable.

## Trust / provenance
- `dedup_report` — `{ removed, kept, overlaps: [{file_a, file_b, count}] }`.
- `period` — `{ start, end }` covered. `currency` — and a guard error if inputs mix
  currencies (the binary refuses to sum across currencies).
