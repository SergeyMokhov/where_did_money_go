# Configuration reference (`profiles.toml`, `categories.toml`, `people.toml`)

The browser app edits these for you (Map columns…, People & accounts, Categories & rules) and
keeps them in each project; this is the format underneath, for hand edits and the CLI.

Three TOML files make up a configuration (a project's `config/` folder, or a CLI config directory). `wdmg init` writes the defaults; edit them by
hand or with the subcommands below. An optional fourth file, `learned.toml`, holds rules the
LLM infers; configs are layered **your files > learned.toml > shipped defaults**.

## `profiles.toml` — how to detect & parse each export
Each `[[profile]]` is tried top-to-bottom; the first whose `match` rules all pass wins.

**Most exports need no profile.** When none matches, the engine works the layout out from the
file itself (`wdmg/src/ingest/infer.rs`) and names it `auto-card-…` or `auto-bank-…`:

- **Columns by their usual names**, compared case- and accent-insensitively with "(GBP)",
  "$" and "." ignored: the date (*Transaction Date*, *Trans. Date*, *Posting Date*,
  *Booking Date*, *Started Date*, …), the amount (*Amount*, *Amount (USD)*, *CAD$*, …) or a
  money-out/money-in pair (*Debit/Credit*, *Money out/Money in*, *Paid out/Paid in*,
  *Withdrawals/Deposits*, *Funds out/Funds in*, …), the description (*Payee*, *Merchant*,
  *Name*, *Counter Party*, *Description*, *Description 1*, …), plus optional memo, type,
  status and cardholder/card-number columns. Common French and German names too.
- **Lines above the header** (an account summary) are skipped, and so are rows without a
  readable date or amount (balance and total lines).
- **No header row:** columns are recognised by contents — the date column, the description
  (longest text), and either one signed amount or two sparse money-out/money-in columns
  (an always-filled third is the balance).
- **Dates:** one format for the whole file, e.g. `25/09` proves day-first so `03/09` is
  3 September; if every date is ambiguous, the order that keeps the rows sorted wins, then
  day-first for GBP/EUR/… and month-first otherwise.
- **Amounts:** `$1,234.56`, `(12.50)`, `12.50-`, `1.234,56` (decimal comma, `;`-separated
  files); a card that writes charges positive (mostly positive amounts, no balance column)
  is flipped; unsigned amounts with a *Debit/Credit* indicator column are signed from it.
- **Cards** (a card/posting-date column, positive charges, or *Sale/Payment* types): money
  in is a transfer (a payment onto the card), as for the built-in card formats. Rows whose
  status is pending, reverted or declined are left out.

A wrongly guessed file can always be described explicitly ("Map columns…" in the app, or a
profile here) — explicit profiles always win.

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
delimiter = ","                       # optional; "," by default
header_row = 0                        # optional; lines of summary above the header to skip
headerless = false                    # optional; no header row: columns are named "1", "2", …
lenient = false                       # optional; skip rows without a readable date/amount
[profile.date]
column  = "Date"
format  = "%m/%d/%Y"                   # or formats = ["%m/%d/%Y", "%d/%m/%Y"]: the one that reads
                                       # every row is used for the whole file (else per row, in order)
[profile.amount]
column = "Amount"
sign   = "signed"                      # signed | outflow_positive | separate_debit_credit
# debit_column / credit_column = "…"   # for separate_debit_credit (either sign is accepted)
# decimal_comma = true                 # amounts like 1.234,56
# direction = { column = "Credit Debit Indicator", outflow = ["Debit"] }  # unsigned amounts
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
[profile.status]                       # optional; rows to leave out
column = "State"
skip   = ["PENDING", "REVERTED"]       # case-insensitive
[profile.account]
kind     = "debit"                     # debit | credit_card | savings | ...
currency = "CAD"
id_from_filename = '(\d{4})'           # capture e.g. a card's last-4 from the filename
```

**Tips.** Profiles match top to bottom and the first full match wins, so put
content-disambiguated profiles (those with `content_probe`) before generic ones with the same
header. Two sources with an identical header are disambiguated by `content_probe` if their
rows differ, otherwise by `filename`. `%-m`/`%-d` parse non-zero-padded month/day. Unmapped
`type` values fall through to category rules — don't force a guess. Files with no type column
can classify by sign: `[profile.type.by_sign] negative = "spend", positive = "income"`; a
description split over several columns is joined with `[profile.merchant] extra_columns`.

## `categories.toml` — how to label & sign transactions
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

## `people.toml` — household & transfer ownership
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

## Editing config from the CLI (lossless TOML round-trip)
```bash
wdmg add-category   --config <dir> --name "Hobbies" --kind spend
wdmg add-rule       --config <dir> --category "Hobbies" --sub "Music" --pattern "MUSIC SHOP" [--min 25] [--max 500] [--priority 5]
wdmg add-person     --config <dir> --id partner --name "Partner"
wdmg assign-account --config <dir> --person partner --id partner_card --profile first_bank_card --kind debit [--cardholder NAME] [--card-last4 1234] [--filename '(?i)foo'] [--external]
wdmg assign-transfer --config <dir> --pattern '(?i)to:\s*partner' --to partner [--priority 8]
```
To remove something, edit the TOML directly (it round-trips cleanly), then re-run `analyze`.

Transfer rules are tried highest `priority` first (then file order) and the first match wins;
they only apply to transfers whose other half isn't in your statements. They are what lets
"INTERAC e-Transfer To: Leela" be attributed to a person, and "Withdrawal to SAVINGS" to an
external savings account. `learned.toml` holds categories only — transfer rules live in
`people.toml`.
