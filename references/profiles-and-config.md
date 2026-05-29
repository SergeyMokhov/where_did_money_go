# Configuring profiles, categories, and people

Read this before editing any config TOML or adding support for a new bank/person.

## Adding a new bank/source (`profiles.toml`)

Append a `[[profile]]`. Profiles match top-to-bottom; first full match wins, so put
content-disambiguated profiles (those with `content_probe`) before generic ones with the
same header.

```toml
[[profile]]
id = "my_bank"
[profile.match]
header_first_cell = "Date"                 # exact, after BOM/quote strip; "A|B" allowed
header_contains   = ["Amount", "Memo"]      # all must appear in header
content_probe     = 'Category:'             # optional regex over first data rows
filename          = "(?i)mybank"            # optional basename regex (disambiguate shared headers)
[profile.encoding]
has_bom = false
[profile.layout]
skip_blank_rows_after_header = 0
delimiter = ","
[profile.date]
column  = "Date"
format  = "%m/%d/%Y"                         # or `formats = [..]` to try several in order
[profile.amount]
column = "Amount"
sign   = "signed"                            # signed | outflow_positive | separate_debit_credit
# debit_column / credit_column required only for separate_debit_credit
[profile.merchant]
column      = "Name"
strip_codes = '^\s*\[(?:CW|PR)\]\s*'         # optional regex prefix to remove
trim        = "trailing_whitespace"          # optional, for fixed-width padding
[profile.memo]
column           = "Memo"
category_harvest = 'Category:\s*(?P<cat>.+?)\s*$'   # optional bank-provided category hint
[profile.type]
column = "Transaction"
map    = { DEBIT = "spend", CREDIT = "income", PAYMENT = "transfer" }
[profile.person]
column = "Card Holder Name"                  # when the export names the cardholder
[profile.account]
kind     = "debit"                           # debit | credit_card | savings | ...
currency = "CAD"
id_from_filename = '(\d{4})'                 # capture e.g. card last-4 from filename
```

Notes:
- Two sources can share an identical header (e.g. a debit ledger and a savings ledger).
  Distinguish them with `content_probe` if the rows differ, otherwise with `filename`, and
  attribute the account/owner via `people.toml`.
- `%-m`/`%-d` parse non-zero-padded month/day (e.g. Rewards `1/20/2020`).
- Unmapped `type` values fall through to category rules; don't force a guess.

## Categories (`categories.toml`)

- `kind = spend|income|transfer`. **transfer** is netted out of both flows — use it for
  credit-card payments and movements between your own accounts/people so nothing is double
  counted.
- Subcategory `patterns` are **unanchored, case-insensitive** regexes over the *normalized*
  merchant. Most-specific match wins: longest matching pattern, then highest `priority`
  (default 0), then config order. Give transfer/CC-payment rules a high `priority` so they
  beat generic merchant matches.
- `amount_min` / `amount_max` are decimal **strings**; a rule only matches if `|amount|` is
  in range. Use for e.g. "coffee is < $20", "gas is ≥ $20".
- Don't edit `learned.toml` defaults by hand unless pruning; it is the LLM overlay.

## People & transfers (`people.toml`)

- Each `[[person]]` lists `[[person.account]]`s, matched by `cardholder`, `card_last4`,
  `filename` regex, and/or `profile`. Mark accounts not present in the exports with
  `external = true` — they receive transfers but have no transactions.
- `[[transfer_rule]]` maps a transfer description `pattern` to a target person/account
  (`to`), highest `priority` first. This is what lets "INTERAC e-Transfer To: Leela" be
  attributed to the spouse, and "Withdrawal to SAVINGS" to an external savings account.
- Unmatched transfer counterparties appear in `review_queue.json` for the host agent to
  assign; confident assignments are auto-written to `learned.toml`.
