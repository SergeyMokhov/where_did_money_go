// "Summary for AI": the least an AI needs to write insights — totals by month,
// category and merchant, recurring charges and unusual-activity flags — without
// the transaction list, bank wording, account names, transfer details or exact
// dates (months only); people become "Person 1", "Person 2", …
// Merchant names stay: advice like "cancel X" needs them.

/* eslint-disable @typescript-eslint/no-explicit-any */
const toMonth = (s: unknown) => String(s ?? '').replace(/\b(\d{4}-\d{2})-\d{2}\b/g, '$1');

export function aiSummary(analysis: any, projectName = ''): Record<string, unknown> {
  const a = analysis || {};
  const people = new Map<string, string>();
  const person = (id: string) => {
    if (!people.has(id)) people.set(id, `Person ${people.size + 1}`);
    return people.get(id)!;
  };
  const cat = (r: any) => ({ category: r.category, subcategory: r.subcategory, total: r.total, count: r.count, pct_of_spend: r.pct_of_spend, monthly: r.monthly });
  return {
    kind: 'wdmg-ai-summary',
    version: 1,
    about: 'Aggregated figures from the where-did-money-go app, for writing insights.json. '
      + 'No transactions, bank descriptions, account names or exact dates are included; people are pseudonymous.',
    project: projectName ? 'shared without its name' : undefined,
    period: { from: toMonth(a.period?.start), to: toMonth(a.period?.end) },
    currency: a.currency,
    totals: { inflow: a.total_inflow, outflow: a.total_outflow, net: a.net, savings_rate: a.savings_rate },
    monthly: (a.monthly || []).map((m: any) => ({ period: m.period, inflow: m.inflow, outflow: m.outflow, net: m.net })),
    spend_by_category: (a.spend_by_category || []).map(cat),
    income_by_source: (a.income_by_source || []).map(cat),
    top_merchants: (a.top_merchants || []).map((m: any) => ({ merchant: m.merchant, total: m.total, count: m.count, avg: m.avg })),
    recurring: (a.recurring || []).map((r: any) => ({
      merchant: r.merchant, cadence: r.cadence, typical_amount: r.typical_amount, occurrences: r.occurrences, monthly_equivalent: r.monthly_equivalent,
    })),
    unusual_activity: (a.anomalies || []).map((x: any) => ({
      method: x.method, category: x.category || undefined, subcategory: x.subcategory || undefined, merchant: x.merchant || undefined,
      period: toMonth(x.period_or_date), amount: x.amount, severity: x.severity, reason: toMonth(x.reason),
    })),
    trends: a.trends || [],
    by_person: (a.by_person || []).map((p: any) => ({ person: person(p.id), spend: p.spend, income: p.income, net: p.net })),
  };
}
