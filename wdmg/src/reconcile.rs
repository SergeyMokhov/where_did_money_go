//! Transfer reconciliation.
//!
//! Operates on transactions whose effective kind is [`EffectiveKind::Transfer`]
//! (credit-card payments, inter-account moves). It does three things, in order:
//!
//! 1. **Internal pairing.** Match an outflow leg (negative) on account/person A
//!    against an inflow leg (positive) on account/person B with equal absolute
//!    amount, dates within ±3 days, and *compatible counterparties* (the
//!    description on one side names the other side's owner, or a CC payment-out
//!    pairs with a "PAYMENT - THANK YOU" in). Greedy, highest-confidence first,
//!    strictly 1:1 — each leg is consumed at most once.
//! 2. **External assignment.** For an unpaired leg, apply the compiled
//!    `people.toml` transfer rules (already sorted by descending priority) to
//!    attribute the other side to a person/account that may be `external`.
//! 3. **Unresolved.** Whatever remains is reported and surfaced to the review
//!    queue as an unassigned transfer counterparty.
//!
//! A flow graph over people/accounts is built from matched + external edges.
//! Amounts are exact [`Decimal`]; confidence is the only float (a score).

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::analytics::Categorized;
use crate::categorize::EffectiveKind;
use crate::config::Config;
use crate::identity::Roster;
use crate::model::Money;

/// Maximum day gap allowed when pairing two internal legs.
const MAX_DAY_GAP: i64 = 3;

/// A matched internal transfer pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedPair {
    /// Source account/person (the outflow leg).
    pub from_account: String,
    /// Destination account/person (the inflow leg).
    pub to_account: String,
    /// Absolute amount moved.
    pub amount: Money,
    /// Outflow leg date.
    pub out_date: String,
    /// Inflow leg date.
    pub in_date: String,
    /// Confidence score in `[0, 1]`.
    pub confidence: String,
}

/// A transfer leg attributed to an external/other account by rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct External {
    /// The account/person the leg came from.
    pub from_account: String,
    /// The target person/account (may be external).
    pub to: String,
    /// Absolute amount.
    pub amount: Money,
    /// Leg date.
    pub date: String,
}

/// A transfer leg that could neither be paired nor assigned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unresolved {
    /// Owning account/person, if known.
    pub account: String,
    /// The (signed) amount of the leg.
    pub amount: Money,
    /// Leg date.
    pub date: String,
    /// The normalized description, for review.
    pub description: String,
}

/// A node in the transfer flow graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    /// Node id (person or account id).
    pub id: String,
    /// Whether this node is an external account.
    pub external: bool,
}

/// A directed, aggregated edge in the transfer graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    /// Source node id.
    pub from: String,
    /// Destination node id.
    pub to: String,
    /// Net amount flowed along the edge.
    pub net_amount: Money,
    /// Number of legs aggregated into the edge.
    pub count: usize,
}

/// The transfer flow graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Graph {
    /// Nodes (people + accounts, including external).
    pub nodes: Vec<GraphNode>,
    /// Aggregated directed edges.
    pub edges: Vec<GraphEdge>,
}

/// The full reconciliation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reconciliation {
    /// Internally matched pairs.
    pub matched_pairs: Vec<MatchedPair>,
    /// Externally assigned legs.
    pub external: Vec<External>,
    /// Unresolved legs (also surfaced to the review queue).
    pub unresolved: Vec<Unresolved>,
    /// Flow graph over people/accounts.
    pub graph: Graph,
}

/// A transfer leg used internally during pairing.
struct Leg<'a> {
    item: Categorized<'a>,
    /// Owning node id (account_id, else owner_person, else "unassigned").
    node: String,
    /// Signed amount.
    amount: Decimal,
    used: bool,
}

impl Leg<'_> {
    fn date(&self) -> jiff::civil::Date {
        self.item.txn.date
    }
    fn desc(&self) -> &str {
        &self.item.txn.normalized_merchant
    }
}

/// Node id for a transaction: prefer account, else owner, else "unassigned".
fn node_of(c: &Categorized<'_>) -> String {
    c.txn
        .account_id
        .clone()
        .or_else(|| c.txn.owner_person.clone())
        .unwrap_or_else(|| "unassigned".into())
}

/// Confidence that two legs are the same internal move. Higher is better.
///
/// Base 0.5 for the structural match (equal magnitude, opposite sign, within the
/// day window). Bonuses for: a credit-card "PAYMENT - THANK YOU" inflow against a
/// payment outflow, the description on one side naming the other side's owner,
/// and same-day timing.
fn confidence(out: &Leg<'_>, inflow: &Leg<'_>, config: &Config) -> f64 {
    let mut score = 0.5f64;

    let in_desc = inflow.desc().to_uppercase();
    let out_desc = out.desc().to_uppercase();

    // CC payment heuristic.
    if in_desc.contains("PAYMENT - THANK YOU") || in_desc.contains("PAYMENT") {
        if out_desc.contains("PAYMENT")
            || out_desc.contains("PC FINANCIAL")
            || out_desc.contains("MASTERCARD")
        {
            score += 0.3;
        } else {
            score += 0.1;
        }
    }

    // Description names the counterparty's owner display name.
    let names = |node: &str| -> Vec<String> {
        let mut v = Vec::new();
        for p in &config.people.persons {
            let owns = p.accounts.iter().any(|a| a.id == node) || p.id == node;
            if owns {
                v.push(p.display_name.to_uppercase());
                v.push(p.id.to_uppercase());
            }
        }
        v
    };
    if names(&inflow.node)
        .iter()
        .any(|n| out_desc.contains(n.as_str()))
        || names(&out.node)
            .iter()
            .any(|n| in_desc.contains(n.as_str()))
    {
        score += 0.2;
    }

    // Same-day bonus.
    if out.date() == inflow.date() {
        score += 0.1;
    }

    score.min(1.0)
}

/// Reconcile all transfer legs in `items`.
#[must_use]
pub fn reconcile(items: &[Categorized<'_>], config: &Config, roster: &Roster) -> Reconciliation {
    // Collect transfer legs.
    let mut legs: Vec<Leg<'_>> = items
        .iter()
        .filter(|c| c.assignment.kind == EffectiveKind::Transfer)
        .map(|c| Leg {
            item: *c,
            node: node_of(c),
            amount: c.txn.amount.amount(),
            used: false,
        })
        .collect();

    // --- 1. Internal pairing (greedy, highest confidence first) ---
    // Enumerate all candidate (out, in) pairs, score, sort desc, consume 1:1.
    struct Cand {
        out: usize,
        inflow: usize,
        conf: f64,
        amount: Decimal,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for i in 0..legs.len() {
        for j in 0..legs.len() {
            if i == j {
                continue;
            }
            let (a, b) = (&legs[i], &legs[j]);
            // a is the outflow (negative), b the inflow (positive).
            if a.amount >= Decimal::ZERO || b.amount <= Decimal::ZERO {
                continue;
            }
            if a.amount.abs() != b.amount {
                continue;
            }
            // Different nodes (a leg cannot pair with itself's account).
            if a.node == b.node {
                continue;
            }
            let gap = b
                .date()
                .since(a.date())
                .map(|s| s.get_days() as i64)
                .unwrap_or(i64::MAX)
                .abs();
            if gap > MAX_DAY_GAP {
                continue;
            }
            let conf = confidence(a, b, config);
            cands.push(Cand {
                out: i,
                inflow: j,
                conf,
                amount: a.amount.abs(),
            });
        }
    }
    // Highest confidence first; ties by smaller indices for determinism.
    cands.sort_by(|x, y| {
        y.conf
            .partial_cmp(&x.conf)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.out.cmp(&y.out))
            .then_with(|| x.inflow.cmp(&y.inflow))
    });

    let mut matched_pairs = Vec::new();
    for cand in &cands {
        if legs[cand.out].used || legs[cand.inflow].used {
            continue;
        }
        legs[cand.out].used = true;
        legs[cand.inflow].used = true;
        matched_pairs.push(MatchedPair {
            from_account: legs[cand.out].node.clone(),
            to_account: legs[cand.inflow].node.clone(),
            amount: Money::new(cand.amount),
            out_date: legs[cand.out].date().to_string(),
            in_date: legs[cand.inflow].date().to_string(),
            confidence: format!("{:.2}", cand.conf),
        });
    }

    // --- 2. External assignment via transfer rules ---
    let mut external = Vec::new();
    let mut unresolved = Vec::new();
    for leg in &legs {
        if leg.used {
            continue;
        }
        let rule = config.transfer_rules.iter().find(|r| {
            r.pattern.is_match(&leg.item.txn.raw_description) || r.pattern.is_match(leg.desc())
        });
        match rule {
            Some(r) => external.push(External {
                from_account: leg.node.clone(),
                to: r.rule.to.clone(),
                amount: Money::new(leg.amount.abs()),
                date: leg.date().to_string(),
            }),
            None => unresolved.push(Unresolved {
                account: leg.node.clone(),
                amount: Money::new(leg.amount),
                date: leg.date().to_string(),
                description: leg.desc().to_string(),
            }),
        }
    }

    // Deterministic ordering of outputs.
    matched_pairs.sort_by(|a, b| {
        a.out_date
            .cmp(&b.out_date)
            .then_with(|| a.from_account.cmp(&b.from_account))
            .then_with(|| a.to_account.cmp(&b.to_account))
            .then_with(|| b.amount.amount().cmp(&a.amount.amount()))
    });
    external.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then_with(|| a.from_account.cmp(&b.from_account))
            .then_with(|| a.to.cmp(&b.to))
            .then_with(|| b.amount.amount().cmp(&a.amount.amount()))
    });
    unresolved.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then_with(|| a.account.cmp(&b.account))
            .then_with(|| a.description.cmp(&b.description))
    });

    let graph = build_graph(&matched_pairs, &external, roster);

    Reconciliation {
        matched_pairs,
        external,
        unresolved,
        graph,
    }
}

/// Build the aggregated transfer graph from matched + external edges.
fn build_graph(matched: &[MatchedPair], external: &[External], roster: &Roster) -> Graph {
    // Aggregate net amount and count per directed (from, to).
    let mut edges: BTreeMap<(String, String), (Decimal, usize)> = BTreeMap::new();
    for p in matched {
        let e = edges
            .entry((p.from_account.clone(), p.to_account.clone()))
            .or_insert((Decimal::ZERO, 0));
        e.0 += p.amount.amount();
        e.1 += 1;
    }
    for x in external {
        let e = edges
            .entry((x.from_account.clone(), x.to.clone()))
            .or_insert((Decimal::ZERO, 0));
        e.0 += x.amount.amount();
        e.1 += 1;
    }

    // Nodes: every roster account + person that appears, plus any edge endpoint.
    let mut node_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (from, to) in edges.keys() {
        node_ids.insert(from.clone());
        node_ids.insert(to.clone());
    }
    let nodes = node_ids
        .into_iter()
        .map(|id| GraphNode {
            external: roster.external_accounts.contains(&id),
            id,
        })
        .collect();

    let edges = edges
        .into_iter()
        .map(|((from, to), (net, count))| GraphEdge {
            from,
            to,
            net_amount: Money::new(net),
            count,
        })
        .collect();

    Graph { nodes, edges }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::categorize::Assignment;
    use crate::model::{Currency, Transaction, TxnType};
    use jiff::civil::{date, Date};
    use std::str::FromStr;

    fn leg(
        desc: &str,
        amount: &str,
        account: &str,
        owner: &str,
        d: Date,
    ) -> (Transaction, Assignment) {
        let t = Transaction {
            raw_description: desc.into(),
            normalized_merchant: desc.to_uppercase(),
            date: d,
            time: None,
            amount: Money::from_str(amount).unwrap(),
            currency: Currency::new("CAD"),
            txn_type: TxnType::Transfer,
            memo: None,
            cardholder: None,
            profile_id: "t".into(),
            source_file: "f.csv".into(),
            owner_person: Some(owner.into()),
            account_id: Some(account.into()),
            harvested_category: None,
            dedup_key: String::new(),
        };
        let a = Assignment {
            category: Some("Transfers".into()),
            subcategory: Some("Internal".into()),
            kind: EffectiveKind::Transfer,
            needs_review: false,
        };
        (t, a)
    }

    fn view<'a>(p: &'a [(Transaction, Assignment)]) -> Vec<Categorized<'a>> {
        p.iter()
            .map(|(t, a)| Categorized {
                txn: t,
                assignment: a,
            })
            .collect()
    }

    #[test]
    fn pairs_cc_payment() {
        let cfg = Config::from_defaults().unwrap();
        let roster = Roster::from_config(&cfg);
        let pairs = vec![
            // Outflow from debit toward the card.
            leg(
                "CARD PAYMENTERCARD PAYMENT",
                "-200.00",
                "ledger_debit",
                "me",
                date(2026, 1, 10),
            ),
            // Inflow on the card.
            leg(
                "PAYMENT - THANK YOU",
                "200.00",
                "mc_7001",
                "me",
                date(2026, 1, 11),
            ),
        ];
        let r = reconcile(&view(&pairs), &cfg, &roster);
        assert_eq!(r.matched_pairs.len(), 1);
        assert_eq!(r.matched_pairs[0].from_account, "ledger_debit");
        assert_eq!(r.matched_pairs[0].to_account, "mc_7001");
        assert_eq!(
            r.matched_pairs[0].amount.amount(),
            rust_decimal_macros::dec!(200.00)
        );
        assert!(r.unresolved.is_empty());
        // Graph has one edge.
        assert_eq!(r.graph.edges.len(), 1);
    }

    #[test]
    fn does_not_pair_when_amount_differs() {
        let cfg = Config::from_defaults().unwrap();
        let roster = Roster::from_config(&cfg);
        let pairs = vec![
            leg(
                "PAYMENT",
                "-200.00",
                "ledger_debit",
                "me",
                date(2026, 1, 10),
            ),
            leg(
                "PAYMENT - THANK YOU",
                "199.00",
                "mc_7001",
                "me",
                date(2026, 1, 11),
            ),
        ];
        let r = reconcile(&view(&pairs), &cfg, &roster);
        assert!(r.matched_pairs.is_empty());
    }

    #[test]
    fn does_not_pair_outside_day_window() {
        let cfg = Config::from_defaults().unwrap();
        let roster = Roster::from_config(&cfg);
        let pairs = vec![
            leg("PAYMENT", "-200.00", "ledger_debit", "me", date(2026, 1, 1)),
            leg(
                "PAYMENT - THANK YOU",
                "200.00",
                "mc_7001",
                "me",
                date(2026, 1, 20),
            ),
        ];
        let r = reconcile(&view(&pairs), &cfg, &roster);
        assert!(r.matched_pairs.is_empty());
    }

    #[test]
    fn external_assignment_by_rule() {
        let cfg = Config::from_defaults().unwrap();
        let roster = Roster::from_config(&cfg);
        // Withdrawal to a savings account that isn't in the exports.
        let pairs = vec![leg(
            "EFT Withdrawal to TFSA SAVINGS",
            "-500.00",
            "ledger_debit",
            "me",
            date(2026, 1, 5),
        )];
        let r = reconcile(&view(&pairs), &cfg, &roster);
        assert_eq!(r.external.len(), 1);
        assert_eq!(r.external[0].to, "spouse_savings");
        assert!(r.unresolved.is_empty());
    }

    #[test]
    fn unresolved_when_no_pair_or_rule() {
        let cfg = Config::from_defaults().unwrap();
        let roster = Roster::from_config(&cfg);
        let pairs = vec![leg(
            "MYSTERY TRANSFER OUT",
            "-75.00",
            "ledger_debit",
            "me",
            date(2026, 1, 5),
        )];
        let r = reconcile(&view(&pairs), &cfg, &roster);
        assert_eq!(r.unresolved.len(), 1);
        assert_eq!(r.unresolved[0].account, "ledger_debit");
    }

    #[test]
    fn pairing_is_one_to_one() {
        let cfg = Config::from_defaults().unwrap();
        let roster = Roster::from_config(&cfg);
        // Two identical outflows, one inflow: only one pair forms.
        let pairs = vec![
            leg(
                "PAYMENT",
                "-100.00",
                "ledger_debit",
                "me",
                date(2026, 1, 10),
            ),
            leg(
                "PAYMENT",
                "-100.00",
                "ledger_debit",
                "me",
                date(2026, 1, 10),
            ),
            leg(
                "PAYMENT - THANK YOU",
                "100.00",
                "mc_7001",
                "me",
                date(2026, 1, 10),
            ),
        ];
        let r = reconcile(&view(&pairs), &cfg, &roster);
        assert_eq!(r.matched_pairs.len(), 1);
        // The other outflow becomes unresolved (no rule matches a bare PAYMENT).
        assert_eq!(r.unresolved.len(), 1);
    }
}
