#!/usr/bin/env python3
"""Generate the Futurama-themed playground statement exports (year 2999).

Deterministic (seeded) — run once, commit the CSVs, use forever. It writes the
six exports under ./data in the four supported layouts and tells a story rich
enough to exercise every feature of the report AND hide a few puzzles for people
to solve with it (see playground/spoilers.md for the answers).

Cast & money (all fictional, set in the year 2999):
  * Philip J. Fry  — Planet Express delivery boy. Modest salary from Professor
    Farnsworth; an irrepressible appetite for GAS-STATION SANDWICHES, comics,
    pizza and the arcade. Owns a Mastercard, a rewards card, and (with Leela) the
    joint chequing + savings.
  * Turanga Leela  — Planet Express captain. The responsible one: better salary,
    saves steadily, dotes on her pet NIBBLER (vet, exotic chow), trains at the
    kendo dojo, and fuels the delivery SHUTTLE with dark-matter fuel.
  * Bender Rodríguez — bending unit. TWO salaries (Planet Express + a second job
    bending girders at IRONWORKS), yet he still drinks himself broke on blackjack,
    e-Transfers to fembots, booze and cigars. His factory cuts him loose mid-2999
    and the wheels come off.
  * Professor Farnsworth — the employer; signs everyone's Planet Express payroll.
  * External counterparties — Zoidberg (whom Fry keeps bailing out), the fembots,
    the casinos, plus RRSP/TFSA investment accounts.

Story beats the report should reveal: recurring salaries & subscriptions; the
rent flipping from income to spend when Fry moves out; a one-time inheritance
spike; travel & jewellery anomalies; cross-file dedup; matched internal transfers;
and Bender's slow-motion bankruptcy.
"""
import csv
import datetime as dt
import os
import random

random.seed(2999)
HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "data")
os.makedirs(DATA, exist_ok=True)

# Row buffers per export ------------------------------------------------------
mc = []   # mastercard_credit (Fry):  Description, Type, Card Holder Name, Date, Time, Amount
rw = []   # rewards_credit (Fry):     Transaction date, Transaction, Name, Memo, Amount
ch = []   # bank_ledger  (joint chequing): Date, Transaction, Name, Memo, Amount
sv = []   # bank_ledger  (joint savings)
fb = []   # first_bank_card (Leela):  card, Type, Date Posted, Amount, Description
bn = []   # bank_ledger  (Bender):    Date, Transaction, Name, Memo, Amount


def amt(x):
    return f"{x:.2f}"


def u(a, b):
    return round(random.uniform(a, b), 2)


def rday(y, m, lo=1, hi=28):
    return dt.date(y, m, random.randint(lo, hi))


def mc_buy(name, dollars, d, kind="PURCHASE"):
    hour = random.randint(7, 21)
    ampm = "AM" if hour < 12 else "PM"
    h12 = hour if 1 <= hour <= 12 else (hour - 12 if hour > 12 else 12)
    sign = -1 if kind == "PURCHASE" else 1
    mc.append([name, kind, "FRY", d.strftime("%m/%d/%Y"),
               f"{h12:02d}:{random.randint(0,59):02d} {ampm}", amt(sign * dollars)])


def rw_buy(name, dollars, d, cat):
    rw.append([d.strftime("%m/%d/%Y"), "DEBIT", name,
               f"Rewards earned: {dollars*0.01:.2f} ~ Category: {cat}", amt(-dollars)])


def ch_row(name, dollars, d, ttype, memo=""):
    ch.append([d.strftime("%m/%d/%Y"), ttype, name, memo, amt(dollars)])


def sv_row(name, dollars, d, ttype, memo=""):
    sv.append([d.strftime("%m/%d/%Y"), ttype, name, memo, amt(dollars)])


def bn_row(name, dollars, d, ttype, memo=""):
    bn.append([d.strftime("%m/%d/%Y"), ttype, name, memo, amt(dollars)])


def fb_row(name, dollars, d, ttype):
    desc = f"[CW]{name}".ljust(48)
    fb.append(["'5550000000007014'", ttype, d.strftime("%Y%m%d"), amt(dollars), desc])


# Merchant pools --------------------------------------------------------------
GROCERS = ["SLURM MART NEWNEWYORK NY", "FISHY JOES MARKET", "BUY N LARGE", "MOMCORP MART"]
RESTOS = ["ELZARS RESTAURANT", "PANUCCIS PIZZA", "JACKRABBITS DINER"]
FUN = ["COMIX EMPORIUM", "FUN CENTER ARCADE", "GAME STOP 3000"]
NIB_FOOD = ["PETOPIA NEWNEWYORK", "NIBBLONIAN CHOW CO"]
CASINOS = ["LUNAR VEGAS CASINO", "BLACKJACK PALACE", "LUCKY 7 SLOTS", "ROBO BET PALACE"]
FEMBOTS = ["ANGELYNE FEMBOT", "BIG BERTHA BOT", "FEMBOT FANTASIES", "LUSTRON ESCORTS"]
BOOZE = ["LOBRAU BREWERY", "DRINKS DEPOT LIQUOR", "SUDS N SPROCKETS BEER"]
CIGARS = ["EL CHEAPO CIGARS", "STOGIE HUT"]


def month_iter(y0, m0, y1, m1):
    y, m = y0, m0
    while (y, m) <= (y1, m1):
        yield y, m
        m += 1
        if m > 12:
            m, y = 1, y + 1


PAY = 1500.00   # fixed monthly credit-card payment so the two legs match exactly
SAV = 500.00    # fixed monthly chequing -> savings move (matched pair)
SHUTTLE_FUEL = 1200.00  # dark-matter fuel per shuttle fill-up (a fixed, findable number)

# Gambling / fembot escalation by year — Bender's descent into ruin.
GAMBLE = {2997: (2, 300, 900), 2998: (3, 1200, 3200), 2999: (4, 2500, 6500)}
FEMBOT = {2997: (1, 150, 400), 2998: (2, 300, 900), 2999: (3, 500, 1500)}

for y, m in month_iter(2997, 1, 2999, 12):
    def day(dd):
        return dt.date(y, m, min(dd, 28))

    # === INCOME: salaries (Professor Farnsworth signs the Planet Express ones) ==
    ch_row("PLANET EXPRESS PAYROLL - FRY", 1400.00, day(1), "CREDIT", "Direct Deposit")
    ch_row("PLANET EXPRESS PAYROLL - FRY", 1400.00, day(15), "CREDIT", "Direct Deposit")
    fb_row("PLANET EXPRESS PAYROLL LEELA", 1500.00, day(1), "CREDIT")
    fb_row("PLANET EXPRESS PAYROLL LEELA", 1500.00, day(15), "CREDIT")
    bn_row("PLANET EXPRESS PAYROLL - BENDER", 1800.00, day(1), "CREDIT", "Direct Deposit")
    # Bender's second job bending girders at the factory — until they fire him.
    if not (y == 2999 and m >= 7):
        bn_row("IRONWORKS BENDING PAYROLL", 2400.00, day(3), "CREDIT", "Direct Deposit")

    # === RENT: Bender pays Fry (2997-98) then Fry moves out & pays Bender (2999) =
    if y <= 2998:
        ch_row("RENT DEPOSIT - BENDER ROOMMATE", 1500.00, day(4), "CREDIT", "Rent from roommate")
        bn_row("RENT TO FRY - SHARED APARTMENT", -1500.00, day(4), "DEBIT", "Monthly rent")
    else:
        ch_row("RENT TO BENDER ROBOT ARMS APTS", -1800.00, day(4), "DEBIT", "Monthly rent")
        bn_row("RENT DEPOSIT - FRY", 1800.00, day(4), "CREDIT", "Rent from tenant")

    # === Household utilities + telecom (joint chequing / Fry's card) ============
    ch_row("DOOP HYDRO", -u(95, 160), day(12), "DEBIT")
    ch_row("EARTH WATER WORKS", -u(45, 70), day(13), "DEBIT")
    mc_buy("MOMCORP MOBILE", u(80, 95), day(18))

    # === Recurring subscriptions (Fry's Mastercard) ============================
    mc_buy("HYPNOTOAD TV", 14.99, day(7))
    mc_buy("SLURMSTREAM", 9.99, day(7))
    mc_buy("EYEPHONE PLUS", 12.99, day(7))

    # === Fry's everyday habits (Mastercard) ====================================
    for _ in range(10):  # GEM: the gas-station sandwich champion
        mc_buy("GAS-N-GULP SANDWICH CO", u(6, 13), rday(y, m))
    for _ in range(10):
        mc_buy("CAFE 3000", u(4, 9), rday(y, m))
    for _ in range(5):
        mc_buy(random.choice(RESTOS), u(18, 70), rday(y, m))
    for _ in range(5):
        mc_buy(random.choice(FUN), u(8, 45), rday(y, m))
    for _ in range(3):
        mc_buy(random.choice(GROCERS), u(40, 140), rday(y, m))
    mc_buy("PLASMA FUEL DEPOT", u(30, 55), rday(y, m))  # Fry's scooter
    # Online buys on the rewards card (with bank-provided Category hints).
    if m % 2 == 0:
        rw_buy("MOMAZON.COM", u(20, 120), day(9), "Merchandise")
        rw_buy("NIMBUS ELECTRONICS", u(30, 200), day(21), "Electronics")
    # Fry keeps bailing out Dr. Zoidberg (poor, perpetually broke friend).
    ch_row("INTERAC E-TRANSFER TO: ZOIDBERG", -u(20, 60), rday(y, m, 5, 24), "OTHER")

    # === Leela's life (first-bank card) ========================================
    fb_row("DARK MATTER FUEL DEPOT", SHUTTLE_FUEL, day(10), "DEBIT")  # GEM: shuttle fuel
    for _ in range(2):
        fb_row(random.choice(NIB_FOOD), -u(25, 60), rday(y, m), "DEBIT")  # Nibbler chow
    if m % 3 == 0:
        fb_row("INTERSPECIES VET CLINIC", -u(90, 260), rday(y, m), "DEBIT")  # Nibbler vet
    fb_row("KENDO DOJO MASTERS", -60.00, day(8), "DEBIT")
    for _ in range(4):
        fb_row(random.choice(GROCERS), -u(40, 120), rday(y, m), "DEBIT")
    for _ in range(3):
        fb_row("CAFE 3000", -u(4, 9), rday(y, m), "DEBIT")
    if m % 4 == 1:
        fb_row(random.choice(["WINNERS", "FORTYNINER SPORTS"]), -u(35, 140), rday(y, m), "DEBIT")
    # Leela occasionally grabs a gas-station sandwich too (but nowhere near Fry).
    if m % 6 == 0:
        fb_row("GAS-N-GULP SANDWICH CO", -u(6, 12), rday(y, m), "DEBIT")

    # === Bender's ruin (his own ledger) ========================================
    n_g, lo_g, hi_g = GAMBLE[y]
    for _ in range(n_g):
        bn_row(random.choice(CASINOS), -u(lo_g, hi_g), rday(y, m), "DEBIT")
    n_f, lo_f, hi_f = FEMBOT[y]
    for _ in range(n_f):
        bn_row(f"INTERAC E-TRANSFER TO: {random.choice(FEMBOTS)}",
               -u(lo_f, hi_f), rday(y, m), "DEBIT")
    for _ in range(6):
        bn_row(random.choice(BOOZE), -u(15, 80), rday(y, m), "DEBIT")
    for _ in range(2):
        bn_row(random.choice(CIGARS), -u(20, 90), rday(y, m), "DEBIT")
    bn_row("BENDING GYM", -29.99, day(8), "DEBIT")
    if m % 5 == 0:  # Bender also likes a gas-station sandwich now and then
        bn_row("GAS-N-GULP SANDWICH CO", -u(6, 12), rday(y, m), "DEBIT")

    # === Household transfers (matched pairs + external investments) =============
    ch_row("BILL PAYMENT - PLANET EXPRESS MASTERCARD", -PAY, day(25), "DEBIT")
    mc_buy("PAYMENT - THANK YOU", PAY, day(25), kind="PAYMENT")
    ch_row("TRANSFER TO SAVINGS", -SAV, day(26), "DEBIT")
    sv_row("TRANSFER FROM CHEQUING", SAV, day(26), "CREDIT")
    ch_row("RRSP CONTRIBUTION", -300.00, day(27), "DEBIT")
    ch_row("TFSA TRANSFER", -200.00, day(27), "DEBIT")
    if m % 2 == 0:
        ch_row("INTERAC E-TRANSFER TO: LEELA", -u(100, 300), day(20), "OTHER")

# === One-off events (anomalies & spikes) =====================================
mc_buy("MARS VEGAS RESORT", 5500.00, dt.date(2998, 8, 12))            # travel anomaly
mc_buy("WONG INDUSTRIES JEWELLERS", 3000.00, dt.date(2999, 2, 14))    # gift anomaly (for Leela)
ch_row("ESTATE OF YANCY FRY SR", 50000.00, dt.date(2999, 6, 15), "CREDIT", "Inheritance cheque")
bn_row("LUNAR VEGAS CASINO", -8000.00, dt.date(2999, 9, 17), "DEBIT", "rock bottom")  # Bender's worst night
# Year-end savings interest.
for yy in (2997, 2998, 2999):
    sv_row("INTEREST PAID", u(2, 20), dt.date(yy, 12, 31), "CREDIT")


# === Writers =================================================================
def write_csv(path, header, rows, quote_all=False):
    with open(path, "w", newline="") as f:
        w = csv.writer(f, quoting=csv.QUOTE_ALL if quote_all else csv.QUOTE_MINIMAL)
        w.writerow(header)
        w.writerows(rows)


def by_date_mdY(r, i):
    return dt.datetime.strptime(r[i], "%m/%d/%Y")


mc.sort(key=lambda r: by_date_mdY(r, 3))
write_csv(os.path.join(DATA, "fry_mastercard_7011.csv"),
          ["Description", "Type", "Card Holder Name", "Date", "Time", "Amount"], mc, quote_all=True)

rw.sort(key=lambda r: by_date_mdY(r, 0))
write_csv(os.path.join(DATA, "fry_rewards_7012.csv"),
          ["Transaction date", "Transaction", "Name", "Memo", "Amount"], rw)

ch.sort(key=lambda r: by_date_mdY(r, 0))
write_csv(os.path.join(DATA, "joint_chequing.csv"),
          ["Date", "Transaction", "Name", "Memo", "Amount"], ch)

sv.sort(key=lambda r: by_date_mdY(r, 0))
write_csv(os.path.join(DATA, "joint_savings.csv"),
          ["Date", "Transaction", "Name", "Memo", "Amount"], sv)

bn.sort(key=lambda r: by_date_mdY(r, 0))
write_csv(os.path.join(DATA, "bender_ledger_3344.csv"),
          ["Date", "Transaction", "Name", "Memo", "Amount"], bn)

# Overlapping "recent" chequing export (last two months of 2999) for the dedup demo.
recent = [r for r in ch if r[0].endswith("/2999") and r[0][:2] in ("11", "12")]
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
      f"({len(recent)} dup'd), {len(sv)} savings, {len(fb)} first-bank, "
      f"{len(bn)} bender rows into {DATA}")
