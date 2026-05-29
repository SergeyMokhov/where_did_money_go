#!/usr/bin/env python3
"""Generate Futurama-themed playground statement exports for where-did-money-go.

Produces CSVs in the four supported layouts under ./data, telling a small story
that exercises every feature of the report:

  * Fry (me) & Leela (spouse) run a joint household; Bender and assorted creatures
    are external counterparties.
  * Salaries from Planet Express land twice a month (recurring income).
  * Bender PAYS RENT TO FRY in 2022-2023 (rent income); then Fry moves out and
    PAYS RENT TO BENDER in 2024-2025 (the rent flips from income to spend).
  * Fry receives a one-time inheritance from the estate of Yancy Fry Sr (a large
    income spike -> anomaly).
  * Subscriptions (Hypnotoad TV, Slurmstream, Bending Gym) recur monthly.
  * Groceries / restaurants / coffee / gas / utilities / telecom as everyday spend.
  * A Mars Vegas resort blowout and a jewellery splurge as spending anomalies.
  * e-Transfers to creatures (Zoidberg, Amy, Hermes, ...) — shown grouped by
    recipient with NO config needed; plus e-transfers to Leela (household).
  * Monthly credit-card payments and chequing->savings moves (matched transfers),
    and RRSP/TFSA contributions (external transfers).
  * An overlapping "recent" chequing file to demonstrate cross-file dedup.

Everything is deterministic (seeded), so re-running yields identical files.
"""
import csv
import datetime as dt
import os
import random

random.seed(1999)
HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "data")
os.makedirs(DATA, exist_ok=True)

# Row buffers per export ------------------------------------------------------
mc = []   # mastercard_credit: Description, Type, Card Holder Name, Date, Time, Amount
rw = []   # rewards_credit:    Transaction date, Transaction, Name, Memo, Amount
ch = []   # bank_ledger (joint chequing): Date, Transaction, Name, Memo, Amount
sv = []   # bank_ledger (joint savings)
fb = []   # first_bank_card (Leela): card, Type, Date Posted, Amount, Description

def amt(x):
    return f"{x:.2f}"

def mc_buy(name, dollars, d, kind="PURCHASE"):
    hour = random.randint(7, 21)
    ampm = "AM" if hour < 12 else "PM"
    h12 = hour if 1 <= hour <= 12 else (hour - 12 if hour > 12 else 12)
    sign = -1 if kind in ("PURCHASE",) else 1
    mc.append([name, kind, "FRY", d.strftime("%m/%d/%Y"),
               f"{h12:02d}:{random.randint(0,59):02d} {ampm}", amt(sign * dollars)])

def rw_buy(name, dollars, d, cat):
    rw.append([d.strftime("%m/%d/%Y"), "DEBIT", name,
               f"Rewards earned: {dollars*0.01:.2f} ~ Category: {cat}", amt(-dollars)])

def ch_row(name, dollars, d, ttype, memo=""):
    ch.append([d.strftime("%m/%d/%Y"), ttype, name, memo, amt(dollars)])

def sv_row(name, dollars, d, ttype, memo=""):
    sv.append([d.strftime("%m/%d/%Y"), ttype, name, memo, amt(dollars)])

def fb_row(name, dollars, d, ttype):
    desc = f"[CW]{name}".ljust(48)
    fb.append(["'5550000000007014'", ttype, d.strftime("%Y%m%d"), amt(dollars), desc])

GROCERS = ["SLURM MART NEWNEWYORK NY", "FISHY JOES MARKET", "BUY N LARGE", "MOMCORP MART"]
RESTOS = ["ELZARS RESTAURANT", "PANUCCIS PIZZA", "JACKRABBITS DINER"]
GAS = ["PLASMA FUEL DEPOT", "PETRO-EARTH STATION"]
SUBS = [("HYPNOTOAD TV", 14.99), ("SLURMSTREAM", 9.99), ("EYEPHONE PLUS", 12.99)]
CREATURES = ["ZOIDBERG", "AMY WONG", "HERMES CONRAD", "NIBBLER", "SCRUFFY", "CALCULON", "URL"]

def month_iter(y0, m0, y1, m1):
    y, m = y0, m0
    while (y, m) <= (y1, m1):
        yield y, m
        m += 1
        if m > 12:
            m, y = 1, y + 1

PAY = 1500.00  # fixed monthly credit-card payment so the two legs match exactly
SAV = 500.00   # fixed monthly chequing -> savings move (matched pair)

for y, m in month_iter(2022, 1, 2025, 12):
    def day(dd):
        return dt.date(y, m, min(dd, 28))

    # --- Income: salaries (Fry to chequing, Leela to her first-bank card) ----
    ch_row("PLANET EXPRESS PAYROLL FRY", 2400.00, day(1), "CREDIT", "Direct Deposit")
    ch_row("PLANET EXPRESS PAYROLL FRY", 2400.00, day(15), "CREDIT", "Direct Deposit")
    fb_row("PAYROLL PLANET EXPRESS", 2600.00, day(15), "CREDIT")

    # --- Rent: Bender pays Fry (2022-23), then Fry pays Bender (2024-25) ------
    if y <= 2023:
        ch_row("RENT DEPOSIT - BENDER", 1500.00, day(3), "CREDIT", "Rent from tenant")
    else:
        ch_row("RENT TO BENDER ROBOT ARMS APTS", -1800.00, day(3), "DEBIT", "Monthly rent")

    # --- Recurring subscriptions + gym (credit card) -------------------------
    for nm, pr in SUBS:
        mc_buy(nm, pr, day(7))
    mc_buy("BENDING GYM", 29.99, day(8))

    # --- Utilities + telecom -------------------------------------------------
    ch_row("DOOP HYDRO", -round(random.uniform(95, 160), 2), day(12), "DEBIT")
    ch_row("EARTH WATER WORKS", -round(random.uniform(45, 70), 2), day(12), "DEBIT")
    mc_buy("MOMCORP MOBILE", round(random.uniform(80, 95), 2), day(18))

    # --- Everyday spend ------------------------------------------------------
    for _ in range(4):
        mc_buy(random.choice(GROCERS), round(random.uniform(55, 165), 2), day(random.randint(2, 27)))
    for _ in range(2):
        mc_buy(random.choice(RESTOS), round(random.uniform(28, 95), 2), day(random.randint(2, 27)))
    for _ in range(3):
        mc_buy("CAFE 3000", round(random.uniform(4, 9), 2), day(random.randint(2, 27)))
    for _ in range(2):
        mc_buy(random.choice(GAS), round(random.uniform(42, 72), 2), day(random.randint(2, 27)))

    # A couple of rewards-card buys (with bank-provided Category hints).
    if m % 3 == 0:
        rw_buy("MOMAZON.COM", round(random.uniform(20, 120), 2), day(9), "Merchandise")
        rw_buy("NIMBUS ELECTRONICS", round(random.uniform(30, 200), 2), day(21), "Electronics")

    # --- e-Transfers: to a creature (unassigned) + sometimes to Leela --------
    ch_row(f"INTERAC E-TRANSFER TO: {random.choice(CREATURES)}",
           -round(random.uniform(40, 380), 2), day(random.randint(5, 24)), "OTHER")
    if m % 2 == 0:
        ch_row("INTERAC E-TRANSFER TO: LEELA", -round(random.uniform(100, 300), 2), day(20), "OTHER")
    # Leela sends the occasional e-transfer to a creature from her card.
    if m % 4 == 0:
        fb_row(f"INTERAC ETRNSFR SENT {random.choice(CREATURES)}",
               -round(random.uniform(30, 150), 2), day(22), "DEBIT")

    # --- Transfers: credit-card payment + chequing->savings (matched pairs) --
    ch_row("BILL PAYMENT - PLANET EXPRESS MASTERCARD", -PAY, day(25), "DEBIT")
    mc_buy("PAYMENT - THANK YOU", PAY, day(25), kind="PAYMENT")
    ch_row("TRANSFER TO SAVINGS", -SAV, day(26), "DEBIT")
    sv_row("TRANSFER FROM CHEQUING", SAV, day(26), "CREDIT")

    # --- External transfers: RRSP / TFSA contributions -----------------------
    ch_row("RRSP CONTRIBUTION", -300.00, day(27), "DEBIT")
    ch_row("TFSA TRANSFER", -200.00, day(27), "DEBIT")

    # Leela groceries / clothing on her card.
    fb_row(random.choice(GROCERS), -round(random.uniform(40, 120), 2), day(11), "DEBIT")
    if m % 3 == 1:
        fb_row("WINNERS", -round(random.uniform(35, 140), 2), day(16), "DEBIT")

# --- One-off events ----------------------------------------------------------
mc_buy("MARS VEGAS RESORT", 5500.00, dt.date(2023, 12, 20))            # travel anomaly
mc_buy("WONG INDUSTRIES JEWELLERS", 3000.00, dt.date(2024, 2, 14))     # gift anomaly
ch_row("ESTATE OF YANCY FRY SR", 50000.00, dt.date(2024, 6, 15), "CREDIT", "Inheritance cheque")  # income spike
# Savings interest sprinkled in.
for yy in (2022, 2023, 2024, 2025):
    sv_row("INTEREST PAID", round(random.uniform(2, 18), 2), dt.date(yy, 12, 31), "CREDIT")

# --- Writers -----------------------------------------------------------------
def write_csv(path, header, rows, quote_all=False):
    with open(path, "w", newline="") as f:
        w = csv.writer(f, quoting=csv.QUOTE_ALL if quote_all else csv.QUOTE_MINIMAL)
        w.writerow(header)
        w.writerows(rows)

# mastercard sorts by date for realism
def by_mc_date(r):
    return dt.datetime.strptime(r[3], "%m/%d/%Y")
mc.sort(key=by_mc_date)
write_csv(os.path.join(DATA, "fry_mastercard_7011.csv"),
          ["Description", "Type", "Card Holder Name", "Date", "Time", "Amount"], mc, quote_all=True)

rw.sort(key=lambda r: dt.datetime.strptime(r[0], "%m/%d/%Y"))
write_csv(os.path.join(DATA, "fry_rewards_7012.csv"),
          ["Transaction date", "Transaction", "Name", "Memo", "Amount"], rw)

ch.sort(key=lambda r: dt.datetime.strptime(r[0], "%m/%d/%Y"))
write_csv(os.path.join(DATA, "joint_chequing.csv"),
          ["Date", "Transaction", "Name", "Memo", "Amount"], ch)

sv.sort(key=lambda r: dt.datetime.strptime(r[0], "%m/%d/%Y"))
write_csv(os.path.join(DATA, "joint_savings.csv"),
          ["Date", "Transaction", "Name", "Memo", "Amount"], sv)

# Overlapping "recent" chequing export (last two months) for the dedup demo.
recent = [r for r in ch if r[0].endswith("/2025") and r[0][:2] in ("11", "12")]
write_csv(os.path.join(DATA, "joint_chequing_recent.csv"),
          ["Date", "Transaction", "Name", "Memo", "Amount"], recent)

# first_bank_card: UTF-8 BOM, two blank rows after header, fixed-width-ish desc.
fb.sort(key=lambda r: r[2])
fb_path = os.path.join(DATA, "leela_firstbank_7014.csv")
with open(fb_path, "w", newline="", encoding="utf-8-sig") as f:
    f.write("First Bank Card,Transaction Type,Date Posted, Transaction Amount,Description\n")
    f.write("\n\n")
    w = csv.writer(f, quoting=csv.QUOTE_MINIMAL)
    for r in fb:
        w.writerow(r)

print(f"wrote {len(mc)} mastercard, {len(rw)} rewards, {len(ch)} chequing "
      f"({len(recent)} dup'd), {len(sv)} savings, {len(fb)} first-bank rows into {DATA}")
