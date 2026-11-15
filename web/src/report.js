// Report UI ported from wdmg/src/report/templates/report.html (glue script).
// Keep algorithms in sync with the Rust analytics (see that file's notes).
import uPlot from 'uplot';
import { createFloatWin } from './txwin';


export function runReport(DATA, OPTS){
  "use strict";
  DATA = DATA || {}; OPTS = OPTS || {};
  var A = DATA.analysis || {};
  var TX = Array.isArray(A.transactions) ? A.transactions : [];
  var RECON = A.transfers || {matched_pairs:[],external:[],unresolved:[],graph:{nodes:[],edges:[]}};
  var CURRENCY = A.currency || "";

  // ---------- life events (vertical markers on the cash-flow chart) ----------
  // Two sources: events baked into analysis.json (from the config `events.toml`,
  // shared by everyone who opens the report) and events you add in this browser
  // (saved to localStorage, private to you). Both are annotation only — they
  // never touch any computed number.
  var EVENTS_KEY = "wdmg.events.v1." + (OPTS.projectId || "default");
  // Per-event hidden state (config + user), persisted so a toggle survives reload.
  var HIDDEN_KEY = "wdmg.events.hidden.v1." + (OPTS.projectId || "default");
  function isIsoDate(s){ return /^\d{4}-\d{2}-\d{2}$/.test(s || ""); }
  // An event is a *period* (shaded band) when it carries a valid end strictly after
  // its start; otherwise it's a *point* (vertical marker). `allEvents` normalizes a
  // missing/invalid/≤start end to "" so this is a simple truthiness check.
  function isPeriod(e){ return !!(e && e.end); }
  // Stable identity for an event — drives its colour and its hidden state, so both
  // stay put across re-renders (independent of list position).
  function evKey(e){ return (e.source || "") + "|" + e.date + "|" + (e.end || "") + "|" + e.label; }
  // A palette of distinct, theme-readable hues; every event gets its own colour
  // (assigned by a hash of its identity, so the chip and the chart band match).
  var EVENT_PALETTE = ["#c98a2c","#8a63d2","#2e8b8b","#c2384b","#3b6ea5","#5a9e3a",
    "#b5562e","#a23ca2","#4d7d9a","#9a7b1f","#7a5cc0","#3a8f5e","#cf6f8a","#6b8e23"];
  function evColor(e){
    var k = evKey(e), h = 0;
    for(var i = 0; i < k.length; i++){ h = (h * 31 + k.charCodeAt(i)) | 0; }
    return EVENT_PALETTE[Math.abs(h) % EVENT_PALETTE.length];
  }
  function hexToRgba(hex, a){
    var m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
    return m ? "rgba(" + parseInt(m[1],16) + "," + parseInt(m[2],16) + "," + parseInt(m[3],16) + "," + a + ")" : hex;
  }
  function hiddenSet(){
    try { var raw = window.localStorage.getItem(HIDDEN_KEY); var arr = raw ? JSON.parse(raw) : [];
      return new Set(Array.isArray(arr) ? arr : []); } catch(e){ return new Set(); }
  }
  function saveHidden(set){
    try { window.localStorage.setItem(HIDDEN_KEY, JSON.stringify(Array.from(set))); } catch(e){}
  }
  // Flip one event's on-chart visibility and persist it.
  function toggleEventHidden(e){
    var s = hiddenSet(), k = evKey(e);
    if(s.has(k)) s.delete(k); else s.add(k);
    saveHidden(s);
  }
  function cfgEvents(){
    return (Array.isArray(A.events) ? A.events : []).map(function(e){
      return { date:e.date, end:e.end || "", label:e.label, note:e.note || "", source:"config" }; });
  }
  function userEvents(){
    try {
      var raw = window.localStorage.getItem(EVENTS_KEY);
      var arr = raw ? JSON.parse(raw) : [];
      return (Array.isArray(arr) ? arr : []).map(function(e){
        return { date:e.date, end:e.end || "", label:e.label, note:e.note || "", source:"user" }; });
    } catch(e){ return []; }
  }
  function saveUserEvents(list){
    try { window.localStorage.setItem(EVENTS_KEY, JSON.stringify(list.map(function(e){
      return { date:e.date, end:e.end || "", label:e.label, note:e.note || "" }; }))); return true; }
    catch(e){ return false; }
  }
  // Config + user events, validated and sorted by start date (stable, ascending).
  // A period's end is kept only when it's a real date strictly after the start;
  // anything else collapses to a point so downstream rendering can trust `e.end`.
  function allEvents(){
    var all = cfgEvents().concat(userEvents()).filter(function(e){
      return e && isIsoDate(e.date) && e.label && String(e.label).trim(); });
    all.forEach(function(e){ if(!(e.end && isIsoDate(e.end) && e.end > e.date)) e.end = ""; });
    all.sort(function(a, b){ return a.date < b.date ? -1 : a.date > b.date ? 1 : 0; });
    // Annotate identity / colour / hidden once, so chips and chart agree and the
    // draw hook needn't re-read localStorage per event.
    var hidden = hiddenSet();
    all.forEach(function(e){
      e.key = evKey(e); e.color = evColor(e); e.fill = hexToRgba(e.color, 0.15);
      e.hidden = hidden.has(e.key);
    });
    return all;
  }
  // Only the events currently shown on the chart (hidden ones toggled off).
  function visibleEvents(){ return allEvents().filter(function(e){ return !e.hidden; }); }

  // ---------- helpers ----------
  function esc(s){ return String(s == null ? "" : s).replace(/[&<>"]/g, function(c){
    return ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;"})[c]; }); }
  // Exact parse of a (<=2dp) decimal string to integer cents. Currency is 2dp,
  // so summing in cents reproduces the Rust Decimal totals exactly.
  function toCents(s){
    s = String(s == null ? "0" : s).trim();
    var neg = false;
    if(s.charAt(0) === "-"){ neg = true; s = s.slice(1); }
    else if(s.charAt(0) === "+"){ s = s.slice(1); }
    var parts = s.split("."), intp = parts[0] || "0", frac = parts[1] || "";
    var cents;
    if(frac.length <= 2){
      frac = (frac + "00").slice(0,2);
      cents = (parseInt(intp,10)||0) * 100 + (parseInt(frac,10)||0);
    } else {
      cents = (parseInt(intp,10)||0) * 100 + Math.round(parseFloat("0." + frac) * 100);
    }
    return neg ? -cents : cents;
  }
  function fmtMoney(cents){
    var neg = cents < 0; var v = Math.abs(Math.round(cents));
    var d = Math.floor(v / 100), c = v % 100;
    return (neg ? "-" : "") + d.toLocaleString("en-CA") + "." + (c < 10 ? "0" : "") + c;
  }
  function dateMs(iso){
    var m = /^(\d{4})-(\d{2})-(\d{2})/.exec(iso || ""); if(!m) return null;
    return Date.UTC(+m[1], (+m[2]) - 1, +m[3]);
  }
  function monthTs(p){
    var m = /^(\d{4})-(\d{2})/.exec(p || ""); if(!m) return null;
    return Date.UTC(+m[1], (+m[2]) - 1, 1) / 1000;
  }
  function uniqSorted(arr){
    var seen = {}, out = [];
    arr.forEach(function(v){ if(v != null && !seen[v]){ seen[v] = 1; out.push(v); } });
    out.sort(); return out;
  }
  // numeric stats (floats; used only for thresholds/labels, mirroring the Rust
  // f64 ratios — money itself is always summed in exact cents above)
  function median(xs){ if(!xs.length) return null; var v = xs.slice().sort(function(a,b){return a-b;});
    var n = v.length; return n % 2 ? v[(n-1)/2] : (v[n/2-1] + v[n/2]) / 2; }
  function quantile(xs, q){ if(!xs.length) return null; var v = xs.slice().sort(function(a,b){return a-b;});
    var n = v.length; if(n === 1) return v[0];
    var h = (n-1) * q, lo = Math.floor(h), frac = h - lo, hi = Math.min(lo+1, n-1);
    return v[lo] + (v[hi] - v[lo]) * frac; }
  function mean(xs){ if(!xs.length) return null; var s = 0; xs.forEach(function(x){s+=x;}); return s/xs.length; }

  // ---------- facets ----------
  var FACETS = {
    persons:       uniqSorted(TX.map(function(t){return t.person;})),
    accounts:      uniqSorted(TX.map(function(t){return t.account;})),
    categories:    uniqSorted(TX.map(function(t){return t.category;})),
    subcategories: uniqSorted(TX.map(function(t){return t.subcategory;})),
    kinds:         uniqSorted(TX.map(function(t){return t.kind;}))
  };
  var MONTHS = uniqSorted(TX.map(function(t){return t.month;}));
  // account -> representative person (for filtering the transfers panel by person)
  var ACCT_PERSON = {};
  TX.forEach(function(t){ if(t.account && !ACCT_PERSON[t.account]) ACCT_PERSON[t.account] = t.person; });

  var FACET_DEFS = [
    {key:"persons",       label:"Person"},
    {key:"accounts",      label:"Account"},
    {key:"categories",    label:"Category"},
    {key:"subcategories", label:"Subcategory"},
    {key:"kinds",         label:"Kind"}
  ];

  // Each facet holds TWO sets: `inc` (keep only these) and `exc` (drop these).
  // They combine freely, so within one facet you can include some values AND
  // exclude others at the same time (e.g. keep {Groceries, Restaurants} but drop
  // a specific subcategory), and the same across different facets.
  function freshFacet(){ return {inc:new Set(), exc:new Set()}; }
  var state = {
    persons:freshFacet(), accounts:freshFacet(), categories:freshFacet(),
    subcategories:freshFacet(), kinds:freshFacet(),
    monthFrom:null, monthTo:null, search:"", searchExclude:"", amtMin:null, amtMax:null,
    // Range negation: when true, the date / amount range is EXCLUDED instead of kept
    // (e.g. drop everything in Dec 2025, or drop every charge between $0 and $20).
    negMonths:false, negAmt:false,
    // Persona lens for the Unusual-activity section; defaults to the configured
    // profile but is switchable live. Forensic flags show only for business/public.
    lens: (A.analysis_profile || "consumer"),
    // Auto-tag the currently-visible anomalies on the cash-flow chart (default on,
    // remembered in the browser). Set just below from localStorage.
    showAnomTags: true
  };
  var ANOMTAGS_KEY = "wdmg.anomtags.v1";
  try { var _at = window.localStorage.getItem(ANOMTAGS_KEY); if(_at !== null) state.showAnomTags = _at === "1"; } catch(e){}
  // Merchant search/exclude are regexes (case-insensitive). Compiled once per
  // rerender; null when empty or invalid (then we fall back to substring match).
  var SEARCH_RE = null, SEARCH_BAD = false, EXCL_RE = null, EXCL_BAD = false;
  function compileSearch(){
    SEARCH_RE = null; SEARCH_BAD = false; EXCL_RE = null; EXCL_BAD = false;
    if(state.search){ try { SEARCH_RE = new RegExp(state.search, "i"); } catch(e){ SEARCH_BAD = true; } }
    if(state.searchExclude){ try { EXCL_RE = new RegExp(state.searchExclude, "i"); } catch(e){ EXCL_BAD = true; } }
  }
  // A value passes a facet when it is not excluded and (no includes are set, or it
  // is one of the included values). Includes and excludes apply together.
  function facetPass(key, val){
    var f = state[key];
    if(f.exc.has(val)) return false;
    if(f.inc.size && !f.inc.has(val)) return false;
    return true;
  }
  function facetActive(key){ var f = state[key]; return f.inc.size > 0 || f.exc.size > 0; }
  // An include-set that already covers every known value of a facet (and excludes
  // nothing) does not actually narrow the view — selecting "all persons" is the
  // same as selecting none. Used only for aggregate-anomaly relevance, so a whole-
  // dataset flag (cash-flow runway, Benford, a total-spend shift) stays visible
  // when the selection is effectively the entire dataset.
  function facetNarrows(key){
    var f = state[key];
    if(f.exc.size > 0) return true;
    if(f.inc.size === 0) return false;
    return f.inc.size < FACETS[key].length;
  }

  function passes(t){
    if(!facetPass("persons", t.person))           return false;
    if(!facetPass("accounts", t.account))         return false;
    if(!facetPass("categories", t.category))      return false;
    if(!facetPass("subcategories", t.subcategory))return false;
    if(!facetPass("kinds", t.kind))               return false;
    // Date range: keep rows inside [from,to] — or, when negated, EXCLUDE that span.
    if(state.monthFrom || state.monthTo){
      var inMonths = (!state.monthFrom || t.month >= state.monthFrom) &&
                     (!state.monthTo   || t.month <= state.monthTo);
      if(state.negMonths ? inMonths : !inMonths) return false;
    }
    // Amount magnitude range: same keep/exclude treatment as the date range.
    if(state.amtMin != null || state.amtMax != null){
      var mag = Math.abs(toCents(t.amount));
      var inAmt = (state.amtMin == null || mag >= state.amtMin) &&
                  (state.amtMax == null || mag <= state.amtMax);
      if(state.negAmt ? inAmt : !inAmt) return false;
    }
    if(state.search){
      if(SEARCH_RE){ if(!SEARCH_RE.test(t.merchant)) return false; }
      else if(t.merchant.toUpperCase().indexOf(state.search.toUpperCase()) === -1) return false;
    }
    if(state.searchExclude){
      if(EXCL_RE){ if(EXCL_RE.test(t.merchant)) return false; }
      else if(t.merchant.toUpperCase().indexOf(state.searchExclude.toUpperCase()) !== -1) return false;
    }
    return true;
  }
  function anyFilterActive(){
    return facetActive("persons") || facetActive("accounts") || facetActive("categories") ||
      facetActive("subcategories") || facetActive("kinds") || state.monthFrom || state.monthTo ||
      state.search || state.searchExclude || state.amtMin != null || state.amtMax != null;
  }

  // ---------- aggregation (mirrors wdmg/src/analytics) ----------
  function spendMag(t){ return t.kind === "spend" ? Math.abs(toCents(t.amount)) : 0; }
  function incomeAmt(t){ return t.kind === "income" ? toCents(t.amount) : 0; }
  function isFlow(t){ return t.kind === "spend" || t.kind === "income"; }

  // A transaction's (inflow, outflow) contribution in cents. Spend → outflow,
  // income → inflow (matching the Rust engine, so transfers are excluded from the
  // headline flows by default). Transfers/unknowns are attributed BY SIGN so that
  // a transfer-only selection still shows real movement instead of zeros.
  function flowParts(t){
    if(t.kind === "income") return [toCents(t.amount), 0];
    if(t.kind === "spend")  return [0, Math.abs(toCents(t.amount))];
    var c = toCents(t.amount); return c >= 0 ? [c, 0] : [0, -c];
  }

  function computeFlows(rows){
    // If the selection has any spend/income, those are the meaningful flows and
    // transfers stay netted out (matches the Rust totals). If it has ONLY
    // transfers/unknowns (e.g. you filtered to category = Transfers), fall back to
    // showing the transfer movement so the cards/chart aren't empty.
    var hasFlow = rows.some(isFlow);
    var mode = hasFlow ? "flow" : "transfer";
    function inc(t){ return hasFlow ? isFlow(t) : (t.kind === "transfer" || t.kind === "unknown"); }

    var inflow = 0, outflow = 0, count = 0, buckets = {};
    rows.forEach(function(t){
      if(!inc(t)) return;
      var p = flowParts(t);
      inflow += p[0]; outflow += p[1]; count += 1;
      var b = buckets[t.month] || (buckets[t.month] = {inflow:0, outflow:0});
      b.inflow += p[0]; b.outflow += p[1];
    });
    var keys = Object.keys(buckets).sort();
    var nets = keys.map(function(k){ return buckets[k].inflow - buckets[k].outflow; });
    var monthly = keys.map(function(k, i){
      var b = buckets[k], net = b.inflow - b.outflow, base = null;
      if(i > 0){ var lo = Math.max(0, i - 3); base = mean(nets.slice(lo, i)); }
      return {period:k, inflow:b.inflow, outflow:b.outflow, net:net, baseline:base};
    });
    var savings = mode === "flow" && inflow > 0 ? (inflow - outflow) / inflow : null;
    return {inflow:inflow, outflow:outflow, net:inflow-outflow, savings:savings,
      monthly:monthly, mode:mode, count:count};
  }

  function byCategory(rows, kind, sideTotal){
    var g = {};
    rows.forEach(function(t){
      if(t.kind !== kind) return;
      var mag = kind === "spend" ? Math.abs(toCents(t.amount)) : toCents(t.amount);
      var key = t.category + "\u241F" + t.subcategory;
      var e = g[key] || (g[key] = {category:t.category, subcategory:t.subcategory, total:0, count:0});
      e.total += mag; e.count += 1;
    });
    var rowsOut = Object.keys(g).map(function(k){
      var e = g[k];
      e.pct = sideTotal > 0 ? (e.total / sideTotal * 100) : 0;
      return e;
    });
    rowsOut.sort(function(a,b){ return b.total - a.total ||
      (a.category < b.category ? -1 : a.category > b.category ? 1 :
       a.subcategory < b.subcategory ? -1 : a.subcategory > b.subcategory ? 1 : 0); });
    return rowsOut;
  }

  function topMerchants(rows, n){
    var g = {};
    rows.forEach(function(t){
      if(t.kind !== "spend") return;
      var e = g[t.merchant] || (g[t.merchant] = {merchant:t.merchant, total:0, count:0});
      e.total += Math.abs(toCents(t.amount)); e.count += 1;
    });
    var out = Object.keys(g).map(function(k){ var e = g[k]; e.avg = e.count ? e.total/e.count : 0; return e; });
    out.sort(function(a,b){ return b.total - a.total || (a.merchant < b.merchant ? -1 : a.merchant > b.merchant ? 1 : 0); });
    return out.slice(0, n);
  }

  function byEntity(rows, field){
    var g = {};
    rows.forEach(function(t){
      if(!isFlow(t)) return;
      var id = t[field] || "unassigned";
      var e = g[id] || (g[id] = {id:id, spend:0, income:0});
      e.spend += spendMag(t); e.income += incomeAmt(t);
    });
    var out = Object.keys(g).sort().map(function(k){ var e = g[k]; e.net = e.income - e.spend; return e; });
    return out;
  }

  var CADENCES = [["weekly",7,2],["biweekly",14,3],["monthly",30,4],["yearly",365,21]];
  function detectRecurring(rows){
    var g = {};
    rows.forEach(function(t){
      if(t.kind !== "spend") return;
      (g[t.merchant] || (g[t.merchant] = [])).push([dateMs(t.date), Math.abs(toCents(t.amount))]);
    });
    var out = [];
    Object.keys(g).forEach(function(merchant){
      var occ = g[merchant].filter(function(o){ return o[0] != null; });
      if(occ.length < 3) return;
      occ.sort(function(a,b){ return a[0] - b[0]; });
      var intervals = [];
      for(var i = 1; i < occ.length; i++) intervals.push(Math.round((occ[i][0] - occ[i-1][0]) / 86400000));
      var medInt = median(intervals); if(medInt == null) return;
      var medDays = Math.round(medInt);
      var cad = null;
      for(var c = 0; c < CADENCES.length; c++){ if(Math.abs(medDays - CADENCES[c][1]) <= CADENCES[c][2]){ cad = CADENCES[c]; break; } }
      if(!cad) return;
      var amounts = occ.map(function(o){ return o[1]; });
      var m = mean(amounts); if(m === 0) return;
      var varc = mean(amounts.map(function(x){ return (x-m)*(x-m); }));
      var cov = Math.sqrt(varc) / Math.abs(m);
      if(cov >= 0.15) return;
      var typical = median(amounts);
      var monthlyEq = medDays > 0 ? typical * 30 / medDays : typical;
      var last = occ[occ.length-1][0];
      var next = last != null ? new Date(last + medDays*86400000).toISOString().slice(0,10) : null;
      out.push({merchant:merchant, cadence:cad[0], interval_days:medDays, typical:typical,
        occurrences:occ.length, monthly_equivalent:monthlyEq, predicted_next:next});
    });
    out.sort(function(a,b){ return b.monthly_equivalent - a.monthly_equivalent ||
      (a.merchant < b.merchant ? -1 : a.merchant > b.merchant ? 1 : 0); });
    return out;
  }

  // Anomalies are computed once by the Rust engine (a battery of detectors tuned by
  // the persona profile) and embedded in the island. The report does NOT recompute
  // them — it filters the precomputed list to the active selection, so every flag is
  // explained identically to how the engine justified it. `scope` decides how a row
  // is filtered: a transaction-scoped flag follows its txn's facets; a month/dataset
  // flag follows only category + month range.
  var ANOM = Array.isArray(A.anomalies) ? A.anomalies : [];
  // The persona lens is a *nested* hierarchy: each level sees everything the
  // simpler levels see, plus more specialized checks. A method's min level is the
  // lowest lens at which it appears (unlisted = 0, shown to everyone).
  //   0 consumer  — fraud + waste + amount outliers (the actionable everyday set)
  //   1 prosumer  — + time-series analytics (seasonal, regime change)
  //   2 business  — + expense/cash forensics (Benford, near-threshold structuring)
  //   3 public    — + round-number payments to individuals (optics/disclosure)
  var LENS_LEVEL = { consumer:0, prosumer:1, business:2, public:3 };
  var METHOD_MIN_LEVEL = {
    seasonal_residual:1, change_point:1,
    benford:2, structuring:2,
    round_number:3
  };
  function methodLevel(method){ return METHOD_MIN_LEVEL[method] || 0; }
  function lensLevel(){ return LENS_LEVEL[state.lens] || 0; }
  function methodVisible(method){ return methodLevel(method) <= lensLevel(); }
  function anomalyMonthInRange(mo){
    if(!(state.monthFrom || state.monthTo)) return true;
    var inM = (!state.monthFrom || mo >= state.monthFrom) && (!state.monthTo || mo <= state.monthTo);
    return state.negMonths ? !inM : inM;
  }
  // Selection context for judging aggregate-anomaly relevance. Aggregate (month/
  // dataset) anomalies are precomputed by the engine over the WHOLE dataset and
  // can't be re-derived client-side for a narrowed view, so instead of ignoring the
  // filters (which made them feel "stuck"), we test relevance against the currently
  // filtered rows: `present` holds every "category|YYYY-MM" still in the selection,
  // and `narrowed` is true once any facet/search/amount/month filter is active.
  var ANOM_CTX = { present: Object.create(null), narrowed: false };
  function refreshAnomCtx(){
    var rows = lastRows || [], present = Object.create(null);
    for(var i = 0; i < rows.length; i++){ present[rows[i].category + "|" + rows[i].month] = 1; }
    ANOM_CTX.present = present;
    ANOM_CTX.narrowed = facetNarrows("persons") || facetNarrows("accounts") ||
      facetNarrows("subcategories") || facetNarrows("categories") || facetNarrows("kinds") ||
      !!state.search || !!state.searchExclude ||
      state.amtMin != null || state.amtMax != null ||
      !!state.monthFrom || !!state.monthTo;
  }
  function anomalyPasses(a){
    if(!methodVisible(a.method)) return false;
    if(a.category && !facetPass("categories", a.category)) return false;
    // A whole-dataset flag (Benford, cash-flow runway) describes the entire dataset;
    // it's only meaningful on the un-narrowed view, so hide it once the user filters.
    if(a.scope === "dataset") return !ANOM_CTX.narrowed;
    if(a.scope === "month"){
      var mo = (a.period_or_date || "").slice(0,7);
      if(a.category){
        // A category-month flag shows only while the filtered selection still
        // contains spend in that category+month (so person/account/category/merchant/
        // amount filters all narrow it through the same index).
        if(!ANOM_CTX.present[a.category + "|" + mo]) return false;
      } else if(ANOM_CTX.narrowed){
        // A category-less month flag (e.g. a shift in TOTAL spending) is a whole-
        // dataset construct anchored at a month — only meaningful on the un-narrowed view.
        return false;
      }
      return anomalyMonthInRange(mo);
    }
    // transaction scope: follow the underlying transaction's facets.
    if(!facetPass("persons", a.person || "unassigned")) return false;
    if(!facetPass("accounts", a.account || "unassigned")) return false;
    if(a.subcategory && !facetPass("subcategories", a.subcategory)) return false;
    if(!anomalyMonthInRange((a.period_or_date || "").slice(0,7))) return false;
    if(state.amtMin != null || state.amtMax != null){
      var mag = Math.abs(toCents(a.amount));
      var inA = (state.amtMin == null || mag >= state.amtMin) && (state.amtMax == null || mag <= state.amtMax);
      if(state.negAmt ? inA : !inA) return false;
    }
    var m = a.merchant || "";
    if(state.search){ if(SEARCH_RE){ if(!SEARCH_RE.test(m)) return false; }
      else if(m.toUpperCase().indexOf(state.search.toUpperCase()) === -1) return false; }
    if(state.searchExclude){ if(EXCL_RE){ if(EXCL_RE.test(m)) return false; }
      else if(m.toUpperCase().indexOf(state.searchExclude.toUpperCase()) !== -1) return false; }
    return true;
  }
  function detectAnomalies(){ return ANOM.filter(anomalyPasses); }

  // ----- chart auto-tags for unusual activity -----
  // Severity → marker colour (an alert palette, distinct from the per-event hues).
  var ANOM_TAG_COLOR = { "sev-high":"#d23b4e", "sev-medium":"#d98a2c", "sev-low":"#9aa0a6" };
  function monthLastDayMs(y, m1){ return Date.UTC(y, m1, 0); } // m1 = 1-based month → last day
  // Resolve an anomaly's `period_or_date` to a chart extent (ms). A single day →
  // point; a month (YYYY-MM) or a streak (YYYY-MM…YYYY-MM) → a span. Returns null
  // when there's no usable date (e.g. Benford's "all").
  function anomalyExtent(a){
    var p = (a.period_or_date || "").trim();
    var d = /^(\d{4})-(\d{2})-(\d{2})$/.exec(p);
    if(d){ var t = Date.UTC(+d[1], +d[2]-1, +d[3]); return { start:t, end:t, point:true }; }
    var r = /^(\d{4})-(\d{2})\s*(?:…|\.\.\.)\s*(\d{4})-(\d{2})$/.exec(p);
    if(r){ return { start: Date.UTC(+r[1], +r[2]-1, 1), end: monthLastDayMs(+r[3], +r[4]), point:false }; }
    var mo = /^(\d{4})-(\d{2})$/.exec(p);
    if(mo){ return { start: Date.UTC(+mo[1], +mo[2]-1, 1), end: monthLastDayMs(+mo[1], +mo[2]), point:false }; }
    return null;
  }
  // The currently-visible anomalies that carry a date/range, as drawable chart tags
  // (so the chart and the Unusual-activity table always agree under lens + filters).
  function anomalyChartTags(){
    return detectAnomalies().map(function(a){
      var ext = anomalyExtent(a); if(!ext) return null;
      return { start:ext.start, end:ext.end, point:ext.point,
        color: ANOM_TAG_COLOR[sevClass(a)] || "#9aa0a6" };
    }).filter(Boolean);
  }

  // The most recent COMPLETE month in `rows`. A trailing month is "partial" when
  // the latest transaction date isn't that month's last day (e.g. the current
  // month so far) — including it makes MoM/YoY look like a crash, so trends drop
  // it. Returns {month, partial, raw} or null.
  function lastCompleteMonth(rows){
    var maxIso = null;
    rows.forEach(function(t){ if(t.date && (maxIso === null || t.date > maxIso)) maxIso = t.date; });
    if(!maxIso) return null;
    var m = /^(\d{4})-(\d{2})-(\d{2})/.exec(maxIso); if(!m) return null;
    var y = +m[1], mo = +m[2], day = +m[3];
    var lastDay = new Date(Date.UTC(y, mo, 0)).getUTCDate(); // day 0 of next month
    var raw = m[1] + "-" + m[2];
    if(day >= lastDay) return { month: raw, partial: false, raw: raw };
    var py = mo > 1 ? y : y - 1, pm = mo > 1 ? mo - 1 : 12;
    return { month: py + "-" + ("0" + pm).slice(-2), partial: true, raw: raw };
  }
  // Trend rows for one kind ("spend"/"income"), grouped by `field` ("category"
  // for spend, "subcategory"/source for income). Months after `cutoff` (the last
  // complete month) are dropped. Each row carries the latest complete month's
  // total plus its month-over-month and year-over-year deltas; the renderer puts
  // a coloured arrow on each delta, so the direction IS the number (no separate
  // slope word that could contradict it). Percentages are suppressed when the
  // baseline is under $1, where a ratio explodes into meaningless thousands of %.
  function detectTrends(rows, kind, field, cutoff){
    var PCT_FLOOR = 100; // cents
    var byKey = {};
    rows.forEach(function(t){ if(t.kind !== kind) return;
      if(cutoff && t.month > cutoff) return; // drop the partial trailing month
      var key = t[field] || t.category || "—";
      var cm = byKey[key] || (byKey[key] = {});
      cm[t.month] = (cm[t.month] || 0) + Math.abs(toCents(t.amount)); });
    var out = [];
    Object.keys(byKey).forEach(function(key){
      var months = byKey[key], keys = Object.keys(months).sort();
      if(!keys.length) return;
      var lastKey = keys[keys.length-1], lastTot = months[lastKey];
      var momAbs=null, momPct=null;
      if(keys.length >= 2){ var prev = months[keys[keys.length-2]];
        momAbs = lastTot - prev;
        momPct = Math.abs(prev) < PCT_FLOOR ? null : (momAbs/prev*100).toFixed(1); }
      var yoyAbs=null, yoyPct=null;
      var ym = /^(\d{4})-(\d{2})/.exec(lastKey);
      if(ym){ var pk = (+ym[1]-1) + "-" + ym[2];
        if(months[pk] != null){ yoyAbs = lastTot - months[pk];
          yoyPct = Math.abs(months[pk]) < PCT_FLOOR ? null : (yoyAbs/months[pk]*100).toFixed(1); } }
      out.push({label:key, last:lastTot, last_month:lastKey,
        mom_abs:momAbs, mom_pct:momPct, yoy_abs:yoyAbs, yoy_pct:yoyPct});
    });
    // Biggest mover first (by |MoM Δ|), rows without a MoM last, then by label.
    out.sort(function(a,b){
      var am = a.mom_abs == null ? -1 : Math.abs(a.mom_abs);
      var bm = b.mom_abs == null ? -1 : Math.abs(b.mom_abs);
      if(am !== bm) return bm - am;
      return a.label < b.label ? -1 : a.label > b.label ? 1 : 0; });
    return out;
  }

  // ---------- renderers ----------
  function renderCards(f){
    var netCls = f.net >= 0 ? "pos" : "neg";
    var html;
    if(f.mode === "transfer"){
      html = card("Transfers in", fmtMoney(f.inflow), "pos") +
        card("Transfers out", fmtMoney(f.outflow), "neg") +
        card("Net moved", fmtMoney(f.net), netCls) +
        card("# transfers", String(f.count), "");
    } else {
      var sr = f.savings == null ? "—" : (f.savings * 100).toFixed(1) + "%";
      var srStyle = f.savings == null ? "" : "color:" + savingsColor(f.savings) + ";";
      var srTip = "Share of money-in you kept (didn't spend): (in − out) ÷ in. " +
        "Coloured green at ~30%+, yellow around 15%, red near 0% or negative.";
      html = card("Money in", fmtMoney(f.inflow), "pos") +
        card("Money out", fmtMoney(f.outflow), "neg") +
        card("Net", fmtMoney(f.net), netCls) +
        card("Savings rate", sr, "", srStyle, srTip);
    }
    document.getElementById("cards").innerHTML = html;
    function card(k,v,cls,style,tip){ return '<div class="card"'+(tip?' title="'+esc(tip)+'"':'')+
      '><div class="k">'+esc(k)+'</div><div class="v '+cls+'"'+(style?' style="'+style+'"':'')+
      '>'+esc(v)+'</div></div>'; }
  }
  // Green (high savings) → yellow → red (none/negative). HSL hue 0..120.
  function savingsColor(rate){
    var t = Math.max(0, Math.min(1, rate / 0.30)); // 30%+ savings = full green
    return "hsl(" + Math.round(t * 120) + ",75%,42%)";
  }

  // Distinguishable segment colours (work on light & dark); cycled per subcategory.
  var PALETTE = ["#3b6ea5","#5b9bd5","#26a69a","#7e57c2","#ef6c00","#c2384b",
    "#8d6e63","#66bb6a","#ab47bc","#ffa726","#42a5f5","#789262","#ec407a","#26c6da"];

  // Hierarchical category → subcategory view. Each category is a header with a
  // stacked bar segmented by subcategory (segment width = share of the side
  // total), and an indented, collapsible breakdown of its subcategories (each
  // bar = share of that category). Colours map segment ↔ subrow.
  // ---------- transaction picks ----------
  // Anything clickable that stands for a set of transactions (a category bar, a
  // merchant row, a chart dot…) registers a pick: a stable key, a label and a
  // predicate. Clicking it shows exactly those transactions — within the current
  // filters — in the floating Transactions window. Clicking it again clears it.
  var PICKS = {};
  var SEL = null; // {key, label, test, time}
  function pickAttr(key, label, test){
    PICKS[key] = {key:key, label:label, test:test};
    return ' data-pick="'+esc(key)+'" tabindex="0" role="button" title="Show these transactions"';
  }
  // `data-qpick` is a quiet pick (the explore view): it updates the window, but
  // a minimized window stays minimized — its dock button pulses with the count.
  function markPicked(){
    document.querySelectorAll("[data-pick],[data-qpick]").forEach(function(el){
      var k = el.getAttribute("data-pick") || el.getAttribute("data-qpick");
      el.classList.toggle("picked", !!SEL && k === SEL.key);
    });
  }
  function selectTx(sel, quiet){
    SEL = (sel && SEL && SEL.key === sel.key) ? null : sel;
    renderTxWin(); markPicked();
    if(!SEL) return;
    if(quiet && TXWIN.isMinimized()){
      var b = TXWIN.dockBtn;
      b.classList.remove("pulse"); void b.offsetWidth; b.classList.add("pulse");
      clearTimeout(b._pulseT); b._pulseT = setTimeout(function(){ b.classList.remove("pulse"); }, 2000);
    } else TXWIN.open();
  }
  function onPickEvent(ev){
    var el = ev.target.closest && ev.target.closest("[data-pick],[data-qpick]");
    if(!el) return;
    var quiet = !el.hasAttribute("data-pick");
    var p = PICKS[el.getAttribute(quiet ? "data-qpick" : "data-pick")];
    if(p) selectTx(p, quiet);
  }
  document.addEventListener("click", onPickEvent);
  document.addEventListener("keydown", function(ev){
    if((ev.key === "Enter" || ev.key === " ") && ev.target.matches && ev.target.matches("[data-pick],[data-qpick]")){
      ev.preventDefault(); onPickEvent(ev);
    }
  });
  // A chart bucket: [startMs, endMs) on the chart's own inclusion rule, one side.
  var MON = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
  function bucketEndMs(startMs, gran){
    var d = new Date(startMs);
    if(gran === "month") return Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1);
    return startMs + (gran === "biweek" ? 14 : gran === "week" ? 7 : 1) * DAY_MS;
  }
  function isoOf(ms){ return new Date(ms).toISOString().slice(0, 10); }
  function pickBucket(startMs, endMs, gran, side){
    var hasFlow = lastRows.some(isFlow);
    var sideLbl = side === "in" ? "Money in" : side === "out" ? "Money out" : "Net";
    var d = new Date(startMs), when;
    if(gran === "month") when = MON[d.getUTCMonth()] + " " + d.getUTCFullYear();
    else if(endMs - startMs <= DAY_MS) when = isoOf(startMs);
    else when = isoOf(startMs) + " → " + isoOf(endMs - DAY_MS);
    selectTx({ key: "chart|" + side + "|" + startMs + "|" + endMs, label: sideLbl + " · " + when, time: true,
      test: function(t){
        var ms = dateMs(t.date); if(ms == null || ms < startMs || ms >= endMs) return false;
        if(!(hasFlow ? isFlow(t) : (t.kind === "transfer" || t.kind === "unknown"))) return false;
        var p = flowParts(t);
        return side === "in" ? p[0] !== 0 : side === "out" ? p[1] !== 0 : true;
      } });
  }

  function renderHier(el, flat, sideTotal, emptyMsg, amtCls, rows, kind){
    if(!flat.length){ el.innerHTML = '<div class="empty">'+esc(emptyMsg)+'</div>'; return; }
    var cats = {};
    flat.forEach(function(r){
      var c = cats[r.category] || (cats[r.category] = {category:r.category, total:0, count:0, subs:[]});
      c.total += r.total; c.count += r.count;
      c.subs.push({subcategory:r.subcategory, total:r.total, count:r.count});
    });
    var list = Object.keys(cats).map(function(k){ return cats[k]; });
    list.sort(function(a,b){ return b.total - a.total || (a.category < b.category ? -1 : 1); });
    el.innerHTML = list.map(function(c){
      c.subs.sort(function(a,b){ return b.total - a.total; });
      var catPct = sideTotal > 0 ? c.total / sideTotal * 100 : 0;
      var segs = c.subs.map(function(s,i){
        var wOverall = sideTotal > 0 ? s.total / sideTotal * 100 : 0;
        var pctCat = c.total > 0 ? s.total / c.total * 100 : 0;
        return '<div class="seg" style="width:'+wOverall.toFixed(3)+'%;background:'+PALETTE[i % PALETTE.length]+'" '+
          'title="'+esc(s.subcategory)+': '+fmtMoney(s.total)+' ('+pctCat.toFixed(1)+'% of '+esc(c.category)+')"></div>';
      }).join("");
      var subrows = c.subs.map(function(s,i){
        var pctCat = c.total > 0 ? s.total / c.total * 100 : 0;
        return '<div class="barrow sub"'+pickAttr(kind+"|"+c.category+"|"+s.subcategory, c.category+" / "+s.subcategory, (function(cat, sub){
            return function(t){ return t.kind === kind && t.category === cat && t.subcategory === sub; }; })(c.category, s.subcategory))+
          '><div class="lbl">↳ '+esc(s.subcategory)+
          ' <span class="muted">· '+s.count+'</span></div>'+
          '<div class="bartrack"><div class="barfill" style="width:'+pctCat.toFixed(2)+'%;background:'+PALETTE[i % PALETTE.length]+'"></div></div>'+
          '<div class="amt"><span class="'+amtCls+'">'+fmtMoney(s.total)+'</span> <span class="muted">('+pctCat.toFixed(1)+'%)</span></div></div>';
      }).join("");
      var subLabel = c.subs.length + ' sub' + (c.subs.length === 1 ? '' : 's');
      return '<div class="catgroup"><div class="cathead barrow"'+pickAttr(kind+"|"+c.category, c.category, (function(cat){
          return function(t){ return t.kind === kind && t.category === cat; }; })(c.category))+'>'+
        '<div class="lbl"><span class="caret" role="button" title="Collapse / expand subcategories">▾</span> '+esc(c.category)+' <span class="muted">· '+subLabel+'</span></div>'+
        '<div class="bartrack stacked">'+segs+'</div>'+
        '<div class="amt"><span class="'+amtCls+'">'+fmtMoney(c.total)+'</span> <span class="muted">('+catPct.toFixed(1)+'%)</span></div></div>'+
        '<div class="subrows">'+subrows+'</div></div>';
    }).join("");
    el.querySelectorAll(".cathead .caret").forEach(function(cr){
      cr.addEventListener("click", function(ev){ ev.stopPropagation(); cr.closest(".catgroup").classList.toggle("collapsed"); });
    });
  }

  function renderSpend(f, rows){
    renderHier(document.getElementById("spend-bars"), byCategory(rows, "spend", f.outflow),
      f.outflow, "No categorized spending in the current selection.", "neg", rows, "spend");
  }

  function renderIncome(f, rows){
    renderHier(document.getElementById("income-panel"), byCategory(rows, "income", f.inflow),
      f.inflow, "No categorized income in the current selection.", "pos", rows, "income");
  }

  function renderByEntity(panelId, field, label, rows){
    var el = document.getElementById(panelId);
    var ents = byEntity(rows, field);
    if(!ents.length){ el.innerHTML = '<div class="empty">No '+esc(label.toLowerCase())+' attribution.</div>'; return; }
    el.innerHTML = '<table><thead><tr><th>'+esc(label)+'</th><th class="num">Spend</th><th class="num">Income</th><th class="num">Net</th></tr></thead><tbody>'+
      ents.map(function(e){ return '<tr'+pickAttr(field+"|"+e.id, label+": "+e.id, function(t){
        return isFlow(t) && (t[field] || "unassigned") === e.id; })+'><td>'+esc(e.id)+'</td><td class="num neg">'+fmtMoney(e.spend)+
        '</td><td class="num pos">'+fmtMoney(e.income)+'</td><td class="num">'+fmtMoney(e.net)+'</td></tr>'; }).join("")+
      '</tbody></table>';
  }

  function merchantPick(m){
    return pickAttr("merchant|"+m, m, function(t){ return t.kind === "spend" && t.merchant === m; });
  }
  function renderMerchants(rows){
    var el = document.getElementById("merchants-panel");
    var m = topMerchants(rows, 20);
    if(!m.length){ el.innerHTML = '<div class="empty">No merchant spend in the current selection.</div>'; return; }
    el.innerHTML = '<table><thead><tr><th>Merchant</th><th class="num">Total</th><th class="num">Count</th><th class="num">Avg</th></tr></thead><tbody>'+
      m.map(function(x){ return '<tr'+merchantPick(x.merchant)+'><td>'+esc(x.merchant)+'</td><td class="num">'+fmtMoney(x.total)+
        '</td><td class="num">'+x.count+'</td><td class="num muted">'+fmtMoney(x.avg)+'</td></tr>'; }).join("")+
      '</tbody></table>';
  }

  // The floating Transactions window: every transaction matching the filter bar,
  // narrowed to the current pick (if any). Capped to keep the DOM responsive.
  var ALL_TXN_CAP = 2000;
  var TXWIN = createFloatWin({ id: "txwin", title: "Transactions", dockLabel: "Transactions",
    storageKey: "wdmg.txwin.v1", host: document.getElementById("report-root") || document.body });
  var TXSORT = { col: "date", dir: -1 };
  var TXCOLS = [["date","Date"],["merchant","Merchant"],["category","Category"],["kind","Kind"],
    ["person","Person"],["account","Account"],["amount","Amount"]];
  function renderTxWin(){
    var list = SEL ? lastRows.filter(SEL.test) : lastRows;
    var f = computeFlows(list);
    TXWIN.setBadge(String(list.length));
    TXWIN.setTitle(SEL ? " · " + SEL.label : (anyFilterActive() ? " · filtered" : " · all"));
    var sums = [];
    if(f.inflow) sums.push('in <b class="pos tw-in">'+fmtMoney(f.inflow)+'</b>');
    if(f.outflow) sums.push('out <b class="neg tw-out">'+fmtMoney(f.outflow)+'</b>');
    if(f.inflow && f.outflow) sums.push('net <b>'+fmtMoney(f.inflow - f.outflow)+'</b>');
    TXWIN.tools.innerHTML =
      (SEL ? '<span class="tw-chip">'+esc(SEL.label)+' <button type="button" class="tw-x" title="Show all filtered transactions">✕</button></span>'
           : '<span class="muted">Click a category, merchant, person or chart dot to narrow this list.</span>')+
      '<span class="tw-sum"><span class="tw-count">'+list.length+'</span> transaction'+(list.length===1?'':'s')+
      (sums.length ? ' · '+sums.join(' · ') : '')+
      (anyFilterActive() ? ' <span class="muted" title="The filter bar applies to this list">· filters on</span>' : '')+'</span>';
    var x = TXWIN.tools.querySelector(".tw-x");
    if(x) x.addEventListener("click", function(){ SEL = null; renderTxWin(); markPicked(); });
    var body = TXWIN.body;
    if(!list.length){ body.innerHTML = '<div class="empty">No transactions in the current selection.</div>'; return; }
    var sorted = list.slice().sort(function(a, b){
      var c = TXSORT.col, x, y;
      if(c === "amount"){ x = toCents(a.amount); y = toCents(b.amount); }
      else if(c === "category"){ x = a.category + "/" + a.subcategory; y = b.category + "/" + b.subcategory; }
      else { x = String(a[c] || ""); y = String(b[c] || ""); }
      var r = (x < y ? -1 : x > y ? 1 : 0) * TXSORT.dir;
      return r || (a.date < b.date ? 1 : a.date > b.date ? -1 : 0);
    });
    var shown = sorted.slice(0, ALL_TXN_CAP);
    body.innerHTML = (sorted.length > ALL_TXN_CAP
        ? '<div class="scope-note">Showing '+ALL_TXN_CAP+' of '+sorted.length+' — narrow the filter or pick to see the rest.</div>' : '')+
      '<table><thead><tr>'+TXCOLS.map(function(c){
        var on = TXSORT.col === c[0];
        return '<th data-sort="'+c[0]+'"'+(c[0] === "amount" ? ' class="num"' : '')+' title="Sort">'+c[1]+
          (on ? (TXSORT.dir < 0 ? ' ▾' : ' ▴') : '')+'</th>';
      }).join("")+'</tr></thead><tbody>'+
      shown.map(function(t){
        var c = toCents(t.amount); var cls = c < 0 ? "neg" : "pos";
        return '<tr><td class="muted">'+esc(t.date)+'</td><td class="mer">'+esc(t.merchant)+
          '</td><td>'+esc(t.category)+' <span class="muted">/ '+esc(t.subcategory)+'</span></td>'+
          '<td><span class="tag">'+esc(t.kind)+'</span></td><td>'+esc(t.person)+'</td><td>'+esc(t.account)+
          '</td><td class="num '+cls+'">'+fmtMoney(c)+'</td></tr>';
      }).join("")+
      '</tbody></table>';
    body.querySelectorAll("th[data-sort]").forEach(function(th){
      th.addEventListener("click", function(){
        var c = th.getAttribute("data-sort");
        TXSORT = TXSORT.col === c ? { col: c, dir: -TXSORT.dir } : { col: c, dir: c === "date" || c === "amount" ? -1 : 1 };
        renderTxWin();
      });
    });
  }

  function renderRecurring(rows){
    var el = document.getElementById("recurring-panel");
    var r = detectRecurring(rows);
    if(!r.length){ el.innerHTML = '<div class="empty">No recurring charges in the current selection.</div>'; return; }
    el.innerHTML = '<table><thead><tr><th>Merchant</th><th>Cadence</th><th class="num">Typical</th><th class="num">Monthly equiv.</th><th class="num">Seen</th><th>Next</th></tr></thead><tbody>'+
      r.map(function(x){ return '<tr'+merchantPick(x.merchant)+'><td>'+esc(x.merchant)+'</td><td><span class="tag">'+esc(x.cadence)+
        '</span></td><td class="num">'+fmtMoney(x.typical)+'</td><td class="num">'+fmtMoney(x.monthly_equivalent)+
        '</td><td class="num">'+x.occurrences+'</td><td class="muted">'+(x.predicted_next?esc(x.predicted_next):"—")+'</td></tr>'; }).join("")+
      '</tbody></table>';
  }

  var METHOD_LABEL = { mad_zscore:"unusual amount", monthly_iqr:"expensive month", rare_large:"rare big purchase",
    duplicate_charge:"duplicate charge", first_seen_large:"new large payee", subscription_increase:"price increase",
    card_testing:"card testing", structuring:"near threshold", round_number:"round-number transfer",
    benford:"Benford anomaly", change_point:"spending shift", seasonal_residual:"seasonal spike",
    cashflow_runway:"deficit streak" };
  // Severity colour by method importance (the raw severity scales differ across
  // detectors, so a single numeric cutoff would mislead). mad_zscore keeps its z.
  var METHOD_SEV = { duplicate_charge:"sev-high", structuring:"sev-high", card_testing:"sev-high",
    cashflow_runway:"sev-high", rare_large:"sev-high", first_seen_large:"sev-medium",
    benford:"sev-medium", change_point:"sev-medium", seasonal_residual:"sev-medium",
    monthly_iqr:"sev-medium", subscription_increase:"sev-low", round_number:"sev-low" };
  var SEV_RANK = { "sev-high":0, "sev-medium":1, "sev-low":2 };
  function sevClass(a){
    if(a.method === "mad_zscore"){ var z = parseFloat(a.severity); return z >= 6 ? "sev-high" : z >= 4 ? "sev-medium" : "sev-low"; }
    return METHOD_SEV[a.method] || "sev-low";
  }
  // Lens selector lives INSIDE this section (it only affects Unusual activity).
  // Built fresh each render; the change handler is re-wired below.
  // Per-tier flag counts within the *current facet selection*: how many anomalies
  // first become visible at each lens level. Evaluated with the lens forced to the
  // widest level so every method passes the lens gate and we can bucket by level.
  function lensTierCounts(){
    var saved = state.lens; state.lens = "public";
    var byLevel = [0, 0, 0, 0];
    ANOM.forEach(function(a){ if(anomalyPasses(a)) byLevel[methodLevel(a.method)]++; });
    state.lens = saved;
    return byLevel;
  }
  function lensControlHtml(){
    var opts = [["consumer","Consumer"],["prosumer","Prosumer"],["business","Business"],["public","Public"]]
      .map(function(o){ return '<option value="'+o[0]+'"'+(o[0]===state.lens?' selected':'')+'>'+o[1]+'</option>'; }).join("");
    var lvl = lensLevel(), bl = lensTierCounts();
    var shown = 0; for(var k = 0; k <= lvl; k++) shown += bl[k];
    // What each higher lens would add, so switching always changes the note.
    var adds = [];
    if(lvl < 1 && bl[1]) adds.push("Prosumer +"+bl[1]+" time-series");
    if(lvl < 2 && bl[2]) adds.push("Business +"+bl[2]+" forensic");
    if(lvl < 3 && bl[3]) adds.push("Public +"+bl[3]+" round-number");
    var note = shown + " flag" + (shown === 1 ? "" : "s") + " shown" +
      (adds.length ? " · " + adds.join(" · ") : (lvl > 0 ? " · widest lens" : ""));
    return '<div class="lens-bar"><label>Lens <select id="lens-select" title="Persona lens — each step adds more specialized checks">'+
      opts+'</select></label> <span class="muted">'+note+'</span></div>';
  }
  function wireLens(el){
    var sel = el.querySelector("#lens-select");
    if(sel) sel.addEventListener("change", function(){
      state.lens = sel.value; renderAnomalies();
      drawChart(); // the chart auto-tags follow the visible anomalies, so refresh them
    });
  }
  // The transactions behind a flag: its merchant / category / subcategory, inside
  // its date or month span. Dataset-wide flags without a date (Benford) have none.
  function anomalyPick(a){
    var ext = anomalyExtent(a);
    if(!a.merchant && !a.category && !ext) return "";
    var lbl = (METHOD_LABEL[a.method] || a.method) + " · " + (a.period_or_date || "") + (a.merchant ? " · " + a.merchant : a.category ? " · " + a.category : "");
    return pickAttr("anom|"+a.method+"|"+a.period_or_date+"|"+(a.merchant||"")+"|"+a.category+"|"+(a.subcategory||""), lbl, function(t){
      if(a.merchant && t.merchant !== a.merchant) return false;
      if(a.category && t.category !== a.category) return false;
      if(a.subcategory && t.subcategory !== a.subcategory) return false;
      if(!a.merchant && !a.category && !isFlow(t)) return false;
      if(ext){ var ms = dateMs(t.date); if(ms == null || ms < ext.start || ms > ext.end) return false; }
      return true;
    });
  }
  function renderAnomalies(){
    var el = document.getElementById("anomalies-panel");
    refreshAnomCtx(); // recompute selection context before filtering the precomputed list
    var a = detectAnomalies().slice();
    if(!a.length){
      var empty = ANOM_CTX.narrowed
        ? "No unusual activity for this filtered selection. Whole-dataset checks (cash-flow, Benford) and other people’s flags only show on the unfiltered view — clear the filters to see them."
        : "Nothing unusual in the current selection.";
      el.innerHTML = lensControlHtml() + '<div class="empty">' + empty + '</div>'; wireLens(el); return;
    }
    // Most serious first, then biggest amount.
    a.sort(function(x, y){
      var rx = SEV_RANK[sevClass(x)], ry = SEV_RANK[sevClass(y)];
      if(rx !== ry) return rx - ry;
      return Math.abs(toCents(y.amount)) - Math.abs(toCents(x.amount));
    });
    el.innerHTML = lensControlHtml() + '<table><thead><tr><th>When/where</th><th>Why</th><th class="num">Amount</th><th>Method</th></tr></thead><tbody>'+
      a.map(function(x){
        var when = esc(x.period_or_date) + (x.merchant ? ' · <span class="muted">'+esc(x.merchant)+'</span>' : "");
        var amt = (x.method === "benford") ? '<span class="muted">—</span>' : fmtMoney(toCents(x.amount));
        return '<tr'+anomalyPick(x)+'><td>'+when+'</td><td class="'+sevClass(x)+'">'+esc(x.reason || x.category)+
        '</td><td class="num">'+amt+'</td><td><span class="tag">'+esc(METHOD_LABEL[x.method]||x.method)+'</span></td></tr>'; }).join("")+
      '</tbody></table>';
    wireLens(el);
  }

  function renderTrendTable(panelId, rows, kind, field, colHeader, risingIsGood, cut){
    var el = document.getElementById(panelId); if(!el) return;
    var t = detectTrends(rows, kind, field, cut ? cut.month : null);
    if(!t.length){ el.innerHTML = '<div class="empty">Not enough history for trends in the current selection.</div>'; return; }
    // A delta is "good" when it moves the right way for this kind: spending down
    // (negative) or income up (positive). Colour + arrow live on the number, so
    // the direction is never separate from — or in conflict with — the value.
    function goodCls(num){
      if(num == null) return "";
      if(num === 0) return "muted";
      return (risingIsGood ? num > 0 : num < 0) ? "pos" : "neg";
    }
    function arrow(num){ return num > 0 ? "↑ " : num < 0 ? "↓ " : "→ "; }
    function deltaCell(v){
      if(v == null) return '<td class="num">—</td>';
      return '<td class="num ' + goodCls(v) + '">' + arrow(v) + fmtMoney(Math.abs(v)) + '</td>';
    }
    function pctCell(v){
      if(v == null) return '<td class="num">—</td>';
      var n = parseFloat(v);
      return '<td class="num ' + goodCls(n) + '">' + arrow(n) + Math.abs(n).toFixed(1) + '%</td>';
    }
    var rowsHtml = t.map(function(x){
      return '<tr'+pickAttr("trend|"+kind+"|"+x.label, x.label, function(t){
        return t.kind === kind && (t[field] || t.category || "—") === x.label; })+'><td>'+esc(x.label)+'</td>'+
        deltaCell(x.mom_abs)+pctCell(x.mom_pct)+deltaCell(x.yoy_abs)+pctCell(x.yoy_pct)+'</tr>'; }).join("");
    var note = 'MoM = latest full month vs the month before · YoY = vs the same month a year earlier.' +
      (cut && cut.partial ? ' The current partial month (' + esc(cut.raw) + ') is excluded so it doesn\'t look like a crash.' : '');
    el.innerHTML = '<table><thead><tr><th>'+esc(colHeader)+
      '</th><th class="num">MoM Δ</th><th class="num">MoM %</th><th class="num">YoY Δ</th><th class="num">YoY %</th></tr></thead><tbody>'+
      rowsHtml + '</tbody></table>' +
      '<div class="muted" style="font-size:12px;margin-top:6px;">'+note+'</div>';
  }
  function renderTrends(rows){
    var cut = lastCompleteMonth(rows);
    // Spending (out) by category; income (in) by source/subcategory. For spending
    // a fall is good (green); for income a rise is good (green).
    renderTrendTable("trends-out-panel", rows, "spend",  "category",    "Category", false, cut);
    renderTrendTable("trends-in-panel",  rows, "income", "subcategory", "Source",   true,  cut);
  }

  // ---------- money between accounts: who each transfer leg went to / came from ----------
  // A leg is one transfer row, keyed by account, date and SIGNED cents (a −383.29
  // to one person and a +383.29 from another on the same day are different legs).
  // Matched pairs and rule-assigned (external) legs know their counterparty; any
  // other leg is "inferred": the e-transfer name read from its description, or
  // "Unmatched" when there is none.
  function sKey(acct, date, cents){ return acct + "|" + date + "|" + cents; }
  var LEGMAP = {};
  (RECON.matched_pairs || []).forEach(function(p){ var c = Math.abs(toCents(p.amount));
    LEGMAP[sKey(p.from_account, p.out_date, -c)] = p.to_account;
    LEGMAP[sKey(p.to_account, p.in_date, c)] = p.from_account; });
  (RECON.external || []).forEach(function(e){ var c = Math.abs(toCents(e.amount));
    LEGMAP[sKey(e.from_account, e.date, e.incoming ? c : -c)] = e.to; });
  var UNMATCHED = "Unmatched";
  function counterOf(t){
    var m = LEGMAP[sKey(t.account, t.date, toCents(t.amount))];
    if(m != null) return { name: m, inferred: false };
    var et = etransferInfo(t.merchant);
    return { name: et ? et.who : UNMATCHED, inferred: true };
  }
  var ACCOUNTS = {}; TX.forEach(function(t){ if(t.account) ACCOUNTS[t.account] = 1; });
  var INFER_TOP = 6;   // inferred counterparties drawn on the circle; the rest → Unmatched
  // Focused account / party (null = the circle overview). Survives filter changes.
  var FOCUS = null;    // {node, path:[cat, sub], all:{col:true}, history:[node…]}
  // A pick made inside the focus view belongs to that view; drop it when leaving.
  function dropFocusPick(){
    if(SEL && SEL.key.indexOf("ff|") === 0){ SEL = null; renderTxWin(); markPicked(); }
  }
  function focusOn(node){
    if(FOCUS && FOCUS.node === node) return;
    dropFocusPick();
    FOCUS = { node: node, path: [], all: {}, history: FOCUS ? FOCUS.history.concat([FOCUS.node]) : [] };
    renderTransfers(lastRows);
  }

  function renderTransfers(rows){
    var el = document.getElementById("transfers-panel");
    // Scope the precomputed reconciliation to the CURRENT filter by keeping only
    // the legs whose underlying transaction survived every active filter (the
    // filtered `rows` already passed every facet, date, amount and merchant regex).
    var legKeys = {};
    rows.forEach(function(t){
      if(t.kind !== "transfer") return;
      legKeys[sKey(t.account, t.date, toCents(t.amount))] = 1;
    });
    function hasLeg(acct, date, amtStr, sign){ return !!legKeys[sKey(acct, date, sign * Math.abs(toCents(amtStr)))]; }
    var pairs = (RECON.matched_pairs || []).filter(function(p){
      return hasLeg(p.from_account, p.out_date, p.amount, -1) || hasLeg(p.to_account, p.in_date, p.amount, 1); });
    var ext = (RECON.external || []).filter(function(e){
      return hasLeg(e.from_account, e.date, e.amount, e.incoming ? 1 : -1); });

    var html = "";
    if(anyFilterActive()) html += '<div class="scope-note">Reflects the current filter.</div>';
    var edgeMap = {}, nodeSet = {}, inferredNodes = {};
    var externalIds = {}; (RECON.graph && RECON.graph.nodes || []).forEach(function(n){ if(n.external) externalIds[n.id] = 1; });
    (RECON.external || []).forEach(function(e){ externalIds[e.to] = 1; });
    // Each edge collects its legs, so a click lists exactly the transactions behind it.
    function addEdge(from, to, cents, legs, inferred){
      nodeSet[from] = 1; nodeSet[to] = 1;
      var k = from + "␟" + to + (inferred ? "␟~" : "");
      var e = edgeMap[k] || (edgeMap[k] = {from:from, to:to, net:0, count:0, legs:{}, inferred:!!inferred});
      e.net += cents; e.count += 1;
      legs.forEach(function(l){ e.legs[l] = 1; });
    }
    function legKey(acct, date, amtStr, sign){ return sKey(acct, date, sign * Math.abs(toCents(amtStr))); }
    pairs.forEach(function(p){ addEdge(p.from_account, p.to_account, Math.abs(toCents(p.amount)),
      [legKey(p.from_account, p.out_date, p.amount, -1), legKey(p.to_account, p.in_date, p.amount, 1)]); });
    // An external leg points the way the money moved (incoming = from the external side).
    ext.forEach(function(e){
      var c = Math.abs(toCents(e.amount)), l = [legKey(e.from_account, e.date, e.amount, e.incoming ? 1 : -1)];
      if(e.incoming) addEdge(e.to, e.from_account, c, l); else addEdge(e.from_account, e.to, c, l);
    });
    // Unresolved legs: drawn dashed to an inferred counterparty. The busiest few get
    // their own dot; the rest share "Unmatched" so the circle stays readable.
    var inf = rows.filter(function(t){ return t.kind === "transfer" && t.account && counterOf(t).inferred; });
    var infVol = {};
    inf.forEach(function(t){ var n = counterOf(t).name; infVol[n] = (infVol[n] || 0) + Math.abs(toCents(t.amount)); });
    var keep = {}; Object.keys(infVol).filter(function(n){ return n !== UNMATCHED; })
      .sort(function(x, y){ return infVol[y] - infVol[x]; }).slice(0, INFER_TOP).forEach(function(n){ keep[n] = 1; });
    inf.forEach(function(t){
      var n = counterOf(t).name; if(!keep[n]) n = UNMATCHED;
      inferredNodes[n] = 1;
      var c = toCents(t.amount), k = [sKey(t.account, t.date, c)];
      if(c < 0) addEdge(t.account, n, -c, k, true); else addEdge(n, t.account, c, k, true);
    });
    // Every account in the selection is on the circle, even with no transfers.
    rows.forEach(function(t){ if(t.account) nodeSet[t.account] = 1; });
    var nodes = Object.keys(nodeSet).sort();
    var edges = Object.keys(edgeMap).map(function(k){ return edgeMap[k]; });

    if(!nodes.length){
      FOCUS = null;
      el.innerHTML = html + '<div class="empty">No accounts in the current selection.</div>'; return; }

    var focusing = !!FOCUS;
    html += '<div class="flow-crumbs" id="flow-crumbs"></div><div class="flow" id="flow-graph"></div>'+
      '<div class="flow-legend"><span class="flow-key"></span> money leaves (red) → arrives (green) · width = amount · '+
      (focusing
        ? 'money in on the left, out on the right · click a category to drill into subcategories and merchants · click an account or person to follow the money there · dashed = inferred (unresolved transfer)'
        : 'narrowing toward where it lands · ○ external account · dashed = inferred from the description · drag the dots · click a ribbon or dot to list its transfers · <b>click a name to explore it</b> '+
          '<button type="button" class="btn small flow-reset" hidden>↺ Reset layout</button>')+'</div>';
    if(pairs.length){
      var pairsTot = pairs.reduce(function(a,p){ return a + Math.abs(toCents(p.amount)); }, 0);
      var pairsBody = '<table><thead><tr><th>From</th><th>To</th><th class="num">Amount</th><th>Out</th><th>In</th><th>Confidence</th></tr></thead><tbody>'+
        pairs.map(function(p){ return '<tr><td>'+esc(p.from_account)+'</td><td>'+esc(p.to_account)+
          '</td><td class="num">'+fmtMoney(toCents(p.amount))+'</td><td class="muted">'+esc(p.out_date)+
          '</td><td class="muted">'+esc(p.in_date)+'</td><td><span class="tag">'+esc(p.confidence)+'</span></td></tr>'; }).join("")+
        '</tbody></table>';
      html += collapsible("Matched transfers", pairs.length + " pairs · " + fmtMoney(pairsTot), pairsBody);
    }
    if(ext.length){
      var extTot = ext.reduce(function(a,e){ return a + Math.abs(toCents(e.amount)); }, 0);
      var extBody = '<table><thead><tr><th>From</th><th>To</th><th class="num">Amount</th><th>Date</th></tr></thead><tbody>'+
        ext.map(function(e){ return '<tr><td>'+esc(e.from_account)+'</td><td>'+(e.incoming ? '← ' : '→ ')+esc(e.to)+
          '</td><td class="num">'+fmtMoney(toCents(e.amount))+'</td><td class="muted">'+esc(e.date)+'</td></tr>'; }).join("")+
        '</tbody></table>';
      html += collapsible("To / from external accounts", ext.length + " · " + fmtMoney(extTot), extBody);
    }
    el.innerHTML = html;
    wireCollapsibles(el);
    if(focusing) drawFocus(rows, externalIds, inferredNodes);
    else drawFlowGraph(nodes, edges, externalIds, inferredNodes);
  }

  // ---------- focus view: one account (or outside party), money in → it → money out ----------
  // A Sankey: sources on the left, the focused node in the middle, destinations on
  // the right. Spend categories drill into subcategories, then merchants (one path
  // at a time); accounts and people refocus. Each column shows its top FOCUS_TOP
  // plus "Other (N)". Every node and band is a pick for the Transactions window.
  var FOCUS_TOP = 8;
  function drawFocus(rows, externalIds, inferredNodes){
    var el = document.getElementById("flow-graph"), crumbs = document.getElementById("flow-crumbs");
    if(!el) return;
    var X = FOCUS.node, isAcct = !!ACCOUNTS[X];
    var mag = function(t){ return Math.abs(toCents(t.amount)); };
    // Which left/right node a row belongs to in this focus (null = not part of it).
    function classify(t){
      if(isAcct){
        if(t.account !== X) return null;
        var c = toCents(t.amount);
        if(t.kind === "spend") return { side: "R", id: "cat|" + t.category, label: t.category, kind: "cat", v: Math.abs(c) };
        if(t.kind === "income"){ var src = t.subcategory || t.category;
          return { side: "L", id: "inc|" + src, label: src, kind: "inc", v: c }; }
        if(t.kind === "transfer"){ var cp = counterOf(t);
          return { side: c < 0 ? "R" : "L", id: "party|" + cp.name, label: cp.name, kind: "party", party: cp.name, inferred: cp.inferred, v: Math.abs(c) }; }
        return { side: c < 0 ? "R" : "L", id: "unc", label: "Unknown kind", kind: "unc", v: Math.abs(c) };
      }
      if(t.kind !== "transfer" || !t.account) return null;
      var p = counterOf(t);
      if(p.name !== X) return null;
      // The account paid X → money comes INTO X from it (left); X paid the account → right.
      return { side: toCents(t.amount) < 0 ? "L" : "R", id: "party|" + t.account, label: t.account, kind: "party", party: t.account, inferred: p.inferred, v: mag(t) };
    }
    function group(list, keyOf){
      var g = {};
      list.forEach(function(t){ var k = keyOf(t); if(!k) return;
        var n = g[k.id] || (g[k.id] = { id: k.id, label: k.label, kind: k.kind, party: k.party, inferred: k.inferred, value: 0, n: 0 });
        n.value += k.v; n.n += 1; if(k.inferred === false) n.inferred = false; });
      return Object.keys(g).map(function(k){ return g[k]; }).filter(function(n){ return n.value > 0; })
        .sort(function(a, b){ return b.value - a.value; });
    }
    var mine = rows.filter(function(t){ return !!classify(t); });
    var L = group(mine, function(t){ var c = classify(t); return c.side === "L" ? c : null; });
    var R = group(mine, function(t){ var c = classify(t); return c.side === "R" ? c : null; });
    var path = FOCUS.path.filter(function(x){ return x != null; });
    // Drill columns under the expanded category / subcategory.
    var cat = path[0], sub = path[1];
    if(cat && !R.some(function(n){ return n.id === "cat|" + cat; })){ cat = sub = null; FOCUS.path = []; }
    var inCat = function(t){ var c = classify(t); return c && c.id === "cat|" + cat; };
    var R2 = cat ? group(mine.filter(inCat), function(t){ return { id: "sub|" + t.subcategory, label: t.subcategory, kind: "sub", v: mag(t) }; }) : null;
    if(sub && !(R2 || []).some(function(n){ return n.id === "sub|" + sub; })){ sub = null; FOCUS.path = [cat]; }
    var R3 = sub ? group(mine.filter(function(t){ return inCat(t) && t.subcategory === sub; }),
      function(t){ return { id: "mer|" + t.merchant, label: t.merchant, kind: "mer", v: mag(t) }; }) : null;

    // Predicates for picks: a column node = rows mapped to it.
    function testFor(col, id){
      if(col === "L" || col === "R") return function(t){ var c = classify(t); return !!c && c.side === col && c.id === id; };
      if(col === "R2") return function(t){ return inCat(t) && "sub|" + t.subcategory === id; };
      return function(t){ return inCat(t) && t.subcategory === sub && "mer|" + t.merchant === id; };
    }
    // Top N + "Other (k)"; an expanded node always stays visible.
    function topN(list, col, keepId){
      if(FOCUS.all[col] || list.length <= FOCUS_TOP + 1) return list.map(function(n){ n.test = testFor(col, n.id); return n; });
      var shown = list.slice(0, FOCUS_TOP), rest = list.slice(FOCUS_TOP);
      var ki = keepId ? rest.findIndex(function(n){ return n.id === keepId; }) : -1;
      if(ki >= 0){ shown.push(rest[ki]); rest.splice(ki, 1); }
      shown.forEach(function(n){ n.test = testFor(col, n.id); });
      var tests = rest.map(function(n){ return testFor(col, n.id); });
      shown.push({ id: "other|" + col, label: "Other (" + rest.length + ")", kind: "other", col: col,
        value: rest.reduce(function(a, n){ return a + n.value; }, 0), n: rest.reduce(function(a, n){ return a + n.n; }, 0),
        test: function(t){ return tests.some(function(f){ return f(t); }); } });
      return shown;
    }
    var cols = [
      { key: "L", nodes: topN(L, "L") },
      { key: "C", nodes: [{ id: "center", label: X, kind: "center", value: 0, n: mine.length,
          test: function(t){ return !!classify(t); } }] },
      { key: "R", nodes: topN(R, "R", cat ? "cat|" + cat : null) },
    ];
    if(R2) cols.push({ key: "R2", nodes: topN(R2, "R2", sub ? "sub|" + sub : null) });
    if(R3) cols.push({ key: "R3", nodes: topN(R3, "R3") });
    var sum = function(ns){ return ns.reduce(function(a, n){ return a + n.value; }, 0); };
    var inTot = sum(cols[0].nodes), outTot = sum(cols[2].nodes);
    cols[1].nodes[0].value = Math.max(inTot, outTot, 1);

    // The level the breadcrumb ends at: the open subcategory, category, or X itself.
    var levelNode = cols[1].nodes[0], levelKey = "ff|" + X + "|C|center";
    if(sub){ cols[3].nodes.forEach(function(nd){ if(nd.id === "sub|" + sub){ levelNode = nd; levelKey = "ff|" + X + "|R2|" + nd.id; } }); }
    else if(cat){ cols[2].nodes.forEach(function(nd){ if(nd.id === "cat|" + cat){ levelNode = nd; levelKey = "ff|" + X + "|R|" + nd.id; } }); }
    // ---- breadcrumb ----
    var crumbHtml = (FOCUS.history.length ? '<button type="button" class="btn small fc-back" title="Back to ' + esc(FOCUS.history[FOCUS.history.length - 1]) + '">◂ Back</button> ' : '') +
      '<a href="#" class="fc-crumb" data-lvl="all">All accounts</a> › <a href="#" class="fc-crumb" data-lvl="0">' + esc(X) + '</a>' +
      (cat ? ' › <a href="#" class="fc-crumb" data-lvl="1">' + esc(cat) + '</a>' : '') +
      (sub ? ' › <span>' + esc(sub) + '</span>' : '') +
      '<span class="muted fc-tot"> · in <b class="pos">' + fmtMoney(inTot) + '</b> · out <b class="neg">' + fmtMoney(outTot) + '</b></span>' +
      ' <button type="button" class="btn small fc-list" title="Show this level’s transactions in the Transactions window">≡ ' +
        levelNode.n + ' transaction' + (levelNode.n === 1 ? '' : 's') + '</button>' +
      (OPTS.onEditPeople && X !== UNMATCHED && !inferredNodes[X]
        ? ' <button type="button" class="btn small fc-edit" title="Rename it, change its owner, or give its statements an account">✎ ' +
          (X === "unassigned" ? "Assign accounts…" : "Rename / owner…") + '</button>' : '');
    crumbs.innerHTML = crumbHtml;
    var list = crumbs.querySelector(".fc-list");
    if(list) list.addEventListener("click", function(){
      var p = PICKS[levelKey];
      if(p && !(SEL && SEL.key === p.key)) selectTx(p); else if(p) TXWIN.open();
    });
    crumbs.querySelectorAll(".fc-crumb").forEach(function(a){
      a.addEventListener("click", function(ev){ ev.preventDefault();
        var lvl = a.getAttribute("data-lvl");
        if(lvl === "all") FOCUS = null; else FOCUS.path = FOCUS.path.slice(0, +lvl);
        dropFocusPick();
        renderTransfers(lastRows); });
    });
    var edit = crumbs.querySelector(".fc-edit");
    if(edit) edit.addEventListener("click", function(){ OPTS.onEditPeople(X === "unassigned" ? undefined : X); });
    var back = crumbs.querySelector(".fc-back");
    if(back) back.addEventListener("click", function(){
      var prev = FOCUS.history.pop(); FOCUS = { node: prev, path: [], all: {}, history: FOCUS.history }; dropFocusPick(); renderTransfers(lastRows); });

    // ---- layout ----
    var W = el.clientWidth || 900, NODE_W = 12, GAP = 8, TOP = 34, BOT = 14;
    var most = Math.max.apply(null, cols.map(function(c){ return c.nodes.length; }));
    var need = TOP + BOT + most * 30;
    var H = inWin(el) ? Math.max(260, el.clientHeight, need) : Math.max(380, Math.min(900, need));
    var labelL = Math.min(200, Math.round(W * 0.2)), labelR = Math.min(220, Math.round(W * 0.22));
    var n = cols.length, x0 = labelL, x1 = W - labelR - NODE_W;
    var step = n > 1 ? (x1 - x0) / (n - 1) : 0;
    cols.forEach(function(c, i){ c.x = Math.round(x0 + i * step); });
    var k = Infinity;
    cols.forEach(function(c){
      var tot = sum(c.nodes); if(!tot) return;
      k = Math.min(k, (H - TOP - BOT - GAP * (c.nodes.length - 1)) / tot);
    });
    if(!isFinite(k) || k <= 0) k = 0;
    // Each node gets at least a label's height of room (tiny flows stay legible);
    // if that no longer fits, the view grows taller instead of overlapping.
    var SLOT = 15;
    cols.forEach(function(c){
      c.hs = c.nodes.map(function(nd){ return Math.max(2, nd.value * k); });
      c.tot = c.hs.reduce(function(a, h){ return a + Math.max(h, SLOT); }, 0) + GAP * Math.max(0, c.nodes.length - 1);
    });
    H = Math.max(H, TOP + BOT + Math.max.apply(null, cols.map(function(c){ return c.tot; })));
    cols.forEach(function(c){
      var y = TOP + Math.max(0, (H - TOP - BOT - c.tot) / 2);
      c.nodes.forEach(function(nd, j){
        var h = c.hs[j], slot = Math.max(h, SLOT);
        nd.y = y + (slot - h) / 2; nd.h = h; nd.x = c.x; nd.outOff = 0; nd.inOff = 0; y += slot + GAP;
      });
    });
    var center = cols[1].nodes[0];

    var svg = svgEl("svg", { viewBox: "0 0 " + W + " " + H, width: W, height: H, "font-family": "inherit", "class": "focus-svg" });
    var defs = svgEl("defs", {}); svg.appendChild(defs);
    var gBands = svgEl("g", {}), gNodes = svgEl("g", {});
    svg.appendChild(gBands); svg.appendChild(gNodes);
    var uid = "fc" + Math.random().toString(36).slice(2, 7), gi = 0;
    function pickKey(col, nd){ return "ff|" + X + "|" + col + "|" + nd.id; }
    function register(col, nd, label){
      var key = pickKey(col, nd);
      PICKS[key] = { key: key, label: label, test: nd.test };
      return key;
    }
    function band(src, dst, v, key, inferred){
      var th = Math.max(1, v * k);
      var xs = src.x + NODE_W, xt = dst.x, ys = src.y + src.outOff, yt = dst.y + dst.inOff;
      src.outOff += th; dst.inOff += th;
      var mx = (xs + xt) / 2;
      var d = "M " + xs + " " + ys + " C " + mx + " " + ys + ", " + mx + " " + yt + ", " + xt + " " + yt +
        " L " + xt + " " + (yt + th) + " C " + mx + " " + (yt + th) + ", " + mx + " " + (ys + th) + ", " + xs + " " + (ys + th) + " Z";
      var gid = uid + (gi++);
      var grad = svgEl("linearGradient", { id: gid, gradientUnits: "userSpaceOnUse", x1: xs, y1: 0, x2: xt, y2: 0 });
      var s0 = svgEl("stop", { offset: "0" }), s1 = svgEl("stop", { offset: "1" });
      s0.setAttribute("style", "stop-color:var(--neg)"); s1.setAttribute("style", "stop-color:var(--pos)");
      grad.appendChild(s0); grad.appendChild(s1); defs.appendChild(grad);
      var g = svgEl("g", { "class": "fband" + (inferred ? " inferred" : ""), "data-qpick": key, tabindex: "-1" });
      var path = svgEl("path", { d: d, fill: "url(#" + gid + ")" });
      var tip = svgEl("title", {}); tip.textContent = (PICKS[key] && PICKS[key].label || "") + ": " + fmtMoney(v);
      g.appendChild(tip); g.appendChild(path); gBands.appendChild(g);
    }
    // Bands: left → centre, centre → right, expanded category → subcategories → merchants.
    cols[0].nodes.forEach(function(nd){ band(nd, center, nd.value, register("L", nd, nd.label + " → " + X), nd.inferred); });
    cols[2].nodes.forEach(function(nd){ band(center, nd, nd.value, register("R", nd, X + " → " + nd.label), nd.inferred); });
    if(R2){
      var catNode = cols[2].nodes.filter(function(nd){ return nd.id === "cat|" + cat; })[0];
      if(catNode) cols[3].nodes.forEach(function(nd){ band(catNode, nd, nd.value, register("R2", nd, X + " · " + cat + " / " + nd.label)); });
    }
    if(R3){
      var subNode = cols[3].nodes.filter(function(nd){ return nd.id === "sub|" + sub; })[0];
      if(subNode) cols[4].nodes.forEach(function(nd){ band(subNode, nd, nd.value, register("R3", nd, X + " · " + sub + " · " + nd.label)); });
    }
    register("C", center, X + " (all)");

    // Nodes + labels.
    var charW = 6.6;
    cols.forEach(function(c, i){
      var leftCol = i === 0, last = i === cols.length - 1;
      var room = leftCol ? labelL - 10 : last ? labelR - 10 : step - NODE_W - 14;
      c.nodes.forEach(function(nd){
        var key = pickKey(c.key, nd);
        var cls = "fnode fn-" + nd.kind + (nd.inferred ? " inferred" : "") + (nd.kind === "party" && externalIds[nd.party] ? " ext" : "") +
          ((nd.kind === "cat" && nd.id === "cat|" + cat) || (nd.kind === "sub" && nd.id === "sub|" + sub) ? " open" : "");
        // A box either navigates (drill / follow the money) or, at a leaf, shows
        // its transactions — never both, so drilling leaves the window alone.
        var navigates = nd.kind === "cat" || nd.kind === "sub" || nd.kind === "other" || nd.kind === "party";
        var ga = { "class": cls, tabindex: "0", role: "button" };
        ga[navigates ? "data-nav" : "data-qpick"] = key;
        var g = svgEl("g", ga);
        var r = svgEl("rect", { x: nd.x, y: nd.y, width: NODE_W, height: nd.h, rx: 2 });
        g.appendChild(r);
        var hint = nd.kind === "cat" || nd.kind === "sub" ? (nd.id === "cat|" + cat || nd.id === "sub|" + sub ? " ▾" : " ▸")
          : nd.kind === "party" ? " ↗" : nd.kind === "other" ? " …" : "";
        var amt = " " + fmtAxisNum(Math.round(nd.value / 100));
        var maxChars = Math.max(6, Math.floor(room / charW) - amt.length - hint.length);
        var name = nd.label.length > maxChars ? nd.label.slice(0, maxChars - 1) + "…" : nd.label;
        var t = svgEl("text", { "class": "fn-lbl", y: (nd.y + nd.h / 2 + 4).toFixed(1) });
        if(c.key === "C"){
          t.setAttribute("x", nd.x + NODE_W / 2); t.setAttribute("y", nd.y - 10); t.setAttribute("text-anchor", "middle");
          t.setAttribute("class", "fn-lbl fn-center-lbl");
          t.textContent = nd.label;
        } else {
          t.setAttribute("x", leftCol ? nd.x - 6 : nd.x + NODE_W + 6);
          t.setAttribute("text-anchor", leftCol ? "end" : "start");
          t.textContent = name + hint;
          var ts = svgEl("tspan", { "class": "fn-amt" }); ts.textContent = amt; t.appendChild(ts);
        }
        var tip = svgEl("title", {});
        tip.textContent = nd.label + ": " + fmtMoney(nd.value) + " · " + nd.n + " transaction" + (nd.n === 1 ? "" : "s") +
          (nd.kind === "cat" ? " — click to see subcategories" : nd.kind === "sub" ? " — click to see merchants" :
           nd.kind === "party" ? " — click to follow the money there" : nd.kind === "other" ? " — click to show all" :
           " — click to show these transactions");
        g.appendChild(tip); g.appendChild(t); gNodes.appendChild(g);
        g.addEventListener("click", function(){
          if(nd.kind === "cat"){ FOCUS.path = cat === nd.id.slice(4) ? [] : [nd.id.slice(4)]; }
          else if(nd.kind === "sub"){ FOCUS.path = sub === nd.id.slice(4) ? [cat] : [cat, nd.id.slice(4)]; }
          else if(nd.kind === "other"){ FOCUS.all[nd.col] = !FOCUS.all[nd.col]; }
          else if(nd.kind === "party"){ focusOn(nd.party); return; }
          else return;
          renderTransfers(lastRows);
        });
        if(navigates) g.addEventListener("keydown", function(ev){
          if(ev.key === "Enter" || ev.key === " "){ ev.preventDefault(); g.dispatchEvent(new MouseEvent("click")); }
        });
      });
    });
    // Column headings.
    [["Money in", cols[0].x, "end", NODE_W * -0.5], ["Money out", cols[2].x, "start", NODE_W * 1.5]].forEach(function(h){
      var t = svgEl("text", { "class": "fc-head", x: h[1] + h[3], y: 14, "text-anchor": h[2] }); t.textContent = h[0]; svg.appendChild(t);
    });
    if(!cols[0].nodes.length){
      var t0 = svgEl("text", { "class": "fc-none", x: cols[0].x, y: H / 2, "text-anchor": "end" }); t0.textContent = "nothing in"; svg.appendChild(t0);
    }
    if(!cols[2].nodes.length){
      var t1 = svgEl("text", { "class": "fc-none", x: cols[2].x, y: H / 2 }); t1.textContent = "nothing out"; svg.appendChild(t1);
    }
    el.innerHTML = "";
    el.appendChild(svg);
    markPicked();
  }

  // A collapsed-by-default section: clickable header (caret + title + summary)
  // over a hidden body. Re-usable for the transfer tables.
  function collapsible(title, summary, bodyHtml){
    return '<div class="collapsible collapsed">'+
      '<h3 class="collapse-head"><span class="caret">▾</span>'+esc(title)+
      ' <span class="muted" style="font-weight:400;">· '+esc(summary)+'</span></h3>'+
      '<div class="collapse-body">'+bodyHtml+'</div></div>';
  }
  function wireCollapsibles(root){
    root.querySelectorAll(".collapse-head").forEach(function(h){
      h.addEventListener("click", function(){ h.parentNode.classList.toggle("collapsed"); });
    });
  }

  // Transfer graph: accounts on a circle (draggable), one tapered ribbon per
  // direction — wide where the money leaves (red), narrow where it lands (green).
  // Tapered + colour-graded edges read direction best (Holten & van Wijk 2009).
  var FLOWPOS_KEY = "wdmg.flowpos.v1." + (OPTS.projectId || "default");
  var FLOWPOS = {};
  try { FLOWPOS = JSON.parse(window.localStorage.getItem(FLOWPOS_KEY) || "{}") || {}; } catch(e){ FLOWPOS = {}; }
  function saveFlowPos(){ try { window.localStorage.setItem(FLOWPOS_KEY, JSON.stringify(FLOWPOS)); } catch(e){} }
  var SVGNS = "http://www.w3.org/2000/svg";
  function svgEl(tag, attrs){
    var e = document.createElementNS(SVGNS, tag);
    for(var k in attrs) e.setAttribute(k, attrs[k]);
    return e;
  }
  function drawFlowGraph(nodes, edges, externalIds, inferredNodes){
    inferredNodes = inferredNodes || {};
    var crumbs = document.getElementById("flow-crumbs"); if(crumbs) crumbs.innerHTML = "";
    var el = document.getElementById("flow-graph");
    if(!el || !nodes.length){ if(el) el.innerHTML = ""; return; }
    var W = el.clientWidth || 900;
    // On the page: height follows width. In a window: use the window's full shape.
    var H = inWin(el) ? Math.max(260, el.clientHeight)
                      : Math.round(Math.max(360, Math.min(640, W * 0.62)));
    var cx = W / 2, cy = H / 2;
    // Accounts sit on an ellipse fitted to the area (room left for side labels).
    var RX = Math.max(70, W / 2 - 150), RY = Math.max(60, H / 2 - 40);
    var PAD = 16;
    var maxAmt = 1, vol = {};
    edges.forEach(function(e){
      if(e.net > maxAmt) maxAmt = e.net;
      vol[e.from] = (vol[e.from] || 0) + e.net; vol[e.to] = (vol[e.to] || 0) + e.net;
    });
    var maxVol = 1; nodes.forEach(function(n){ if((vol[n] || 0) > maxVol) maxVol = vol[n]; });
    var P = {};
    nodes.forEach(function(n, i){
      var f = FLOWPOS[n];
      if(f && isFinite(f[0]) && isFinite(f[1])) P[n] = { x: f[0] * W, y: f[1] * H };
      else {
        var ang = -Math.PI / 2 + i * 2 * Math.PI / nodes.length;
        P[n] = { x: cx + RX * Math.cos(ang), y: cy + RY * Math.sin(ang) };
      }
      P[n].r = 5 + 9 * Math.sqrt((vol[n] || 0) / maxVol);
    });

    var svg = svgEl("svg", { viewBox: "0 0 " + W + " " + H, width: W, height: H, "font-family": "inherit" });
    var defs = svgEl("defs", {}); svg.appendChild(defs);
    var gEdges = svgEl("g", {}), gLabels = svgEl("g", {}), gNodes = svgEl("g", {});
    svg.appendChild(gEdges); svg.appendChild(gLabels); svg.appendChild(gNodes);
    var uid = "fg" + Math.random().toString(36).slice(2, 7);

    var E = edges.filter(function(e){ return e.from !== e.to && P[e.from] && P[e.to]; }).map(function(e, i){
      var grad = svgEl("linearGradient", { id: uid + i, gradientUnits: "userSpaceOnUse" });
      var s0 = svgEl("stop", { offset: "0" }), s1 = svgEl("stop", { offset: "1" });
      s0.setAttribute("style", "stop-color:var(--neg)"); s1.setAttribute("style", "stop-color:var(--pos)");
      grad.appendChild(s0); grad.appendChild(s1); defs.appendChild(grad);
      var legs = e.legs, ekey = "flow|" + e.from + "|" + e.to + (e.inferred ? "|~" : "");
      var g = svgEl("g", { "class": "edge-g" + (e.inferred ? " inferred" : ""), "data-pick": ekey, tabindex: "0", role: "button" });
      PICKS[ekey] = { key: ekey, label: "Transfers " + e.from + " → " + e.to + (e.inferred ? " (inferred)" : ""),
        test: function(t){ return t.kind === "transfer" && !!legs[t.account + "|" + t.date + "|" + toCents(t.amount)]; } };
      var tip = svgEl("title", {}); tip.textContent = e.from + " → " + e.to + ": " + fmtMoney(e.net) + " in " + e.count + " transfer" + (e.count === 1 ? "" : "s") +
        (e.inferred ? " (inferred from the description — no matching account or rule)" : "");
      var ribbon = svgEl("path", { "class": "ribbon", fill: "url(#" + uid + i + ")" });
      var hit = svgEl("path", { "class": "edge-hit" });
      g.appendChild(tip); g.appendChild(ribbon); g.appendChild(hit); gEdges.appendChild(g);
      var lbl = svgEl("text", { "class": "amt", "text-anchor": "middle" });
      lbl.textContent = fmtAxisNum(Math.round(e.net / 100)) + " (" + e.count + ")";
      gLabels.appendChild(lbl);
      return { e: e, g: g, grad: grad, ribbon: ribbon, hit: hit, lbl: lbl, w: 2.5 + 13 * Math.sqrt(e.net / maxAmt) };
    });

    function layoutEdge(x){
      var a = P[x.e.from], b = P[x.e.to];
      var dx = b.x - a.x, dy = b.y - a.y, len = Math.hypot(dx, dy) || 1;
      var ux = dx / len, uy = dy / len, nx = -uy, ny = ux;   // A→B bends to its left; B→A to the other side
      var sx = a.x + ux * a.r, sy = a.y + uy * a.r, ex = b.x - ux * (b.r + 3), ey = b.y - uy * (b.r + 3);
      // Both directions bow toward the middle of the graph (so curves stay inside it),
      // split apart by an equal-and-opposite sideways offset (A→B left, B→A right).
      var mx = (sx + ex) / 2, my = (sy + ey) / 2;
      var tcx = cx - mx, tcy = cy - my, tcl = Math.hypot(tcx, tcy) || 1;
      var qx = mx + tcx / tcl * 0.14 * len + nx * (0.13 * len + 12);
      var qy = my + tcy / tcl * 0.14 * len + ny * (0.13 * len + 12);
      // A quadratic curve stays inside the hull of its 3 points: clamp the control point.
      qx = Math.max(PAD, Math.min(W - PAD, qx)); qy = Math.max(PAD, Math.min(H - PAD, qy));
      var N = 28, left = [], right = [], center = [];
      for(var k = 0; k <= N; k++){
        var t = k / N, it = 1 - t;
        var px = it * it * sx + 2 * it * t * qx + t * t * ex, py = it * it * sy + 2 * it * t * qy + t * t * ey;
        var tx = 2 * it * (qx - sx) + 2 * t * (ex - qx), ty = 2 * it * (qy - sy) + 2 * t * (ey - qy);
        var tl = Math.hypot(tx, ty) || 1, ox = -ty / tl, oy = tx / tl;
        var hw = x.w * (1 - 0.82 * t) / 2 + 0.6;
        left.push((px + ox * hw).toFixed(1) + " " + (py + oy * hw).toFixed(1));
        right.push((px - ox * hw).toFixed(1) + " " + (py - oy * hw).toFixed(1));
        center.push(px.toFixed(1) + " " + py.toFixed(1));
        if(k === Math.round(N * 0.45)){
          var lo = x.w / 2 + 9;
          x.lbl.setAttribute("x", Math.max(30, Math.min(W - 30, px + nx * lo)).toFixed(1));
          x.lbl.setAttribute("y", Math.max(12, Math.min(H - 4, py + ny * lo + 4)).toFixed(1));
        }
      }
      x.ribbon.setAttribute("d", "M " + left.join(" L ") + " L " + right.reverse().join(" L ") + " Z");
      x.hit.setAttribute("d", "M " + center.join(" L "));
      x.grad.setAttribute("x1", sx.toFixed(1)); x.grad.setAttribute("y1", sy.toFixed(1));
      x.grad.setAttribute("x2", ex.toFixed(1)); x.grad.setAttribute("y2", ey.toFixed(1));
    }

    var N_ = {};
    nodes.forEach(function(n){
      var touching = E.filter(function(x){ return x.e.from === n || x.e.to === n; });
      var key = "flownode|" + n;
      PICKS[key] = { key: key, label: "Transfers to / from " + n, test: function(t){
        if(t.kind !== "transfer") return false;
        var k = t.account + "|" + t.date + "|" + toCents(t.amount);
        return touching.some(function(x){ return !!x.e.legs[k]; }); } };
      var g = svgEl("g", { "class": "node-g" + (externalIds[n] ? " ext" : "") + (inferredNodes[n] && !ACCOUNTS[n] ? " party" : ""), "data-pick": key, tabindex: "0", role: "button" });
      var tip = svgEl("title", {}); tip.textContent = n + " — drag to move, click the dot to list its transfers, click the name to explore it";
      var c = svgEl("circle", { r: P[n].r.toFixed(1), "class": "node" });
      var t = svgEl("text", { "class": "node-lbl link" }); t.textContent = n;
      // The name opens the focus view (and must not also pick the dot).
      t.addEventListener("click", function(ev){ ev.stopPropagation(); if(Date.now() - dragEnd >= 350) focusOn(n); });
      g.appendChild(tip); g.appendChild(c); g.appendChild(t); gNodes.appendChild(g);
      N_[n] = { g: g, c: c, t: t };
    });
    function layoutNode(n){
      var p = P[n], o = N_[n];
      o.c.setAttribute("cx", p.x.toFixed(1)); o.c.setAttribute("cy", p.y.toFixed(1));
      // Label radially outward from the centre, so it never sits on the ribbons.
      var ax = p.x - cx, ay = p.y - cy, al = Math.hypot(ax, ay) || 1, off = p.r + 7;
      var lx = p.x + ax / al * off, ly = p.y + ay / al * off;
      o.t.setAttribute("x", lx.toFixed(1)); o.t.setAttribute("y", (ly + 4).toFixed(1));
      o.t.setAttribute("text-anchor", ax / al > 0.3 ? "start" : ax / al < -0.3 ? "end" : "middle");
      if(Math.abs(ax / al) <= 0.3) o.t.setAttribute("y", (ly + (ay > 0 ? 12 : -3)).toFixed(1));
    }
    function layoutAll(){ nodes.forEach(layoutNode); E.forEach(layoutEdge); }
    layoutAll();
    el.innerHTML = "";
    el.appendChild(svg);

    var resetBtn = el.parentNode && el.parentNode.querySelector(".flow-reset");
    function syncReset(){ if(resetBtn) resetBtn.hidden = !nodes.some(function(n){ return FLOWPOS[n]; }); }
    syncReset();
    if(resetBtn) resetBtn.onclick = function(){ nodes.forEach(function(n){ delete FLOWPOS[n]; }); saveFlowPos(); drawFlowGraph(nodes, edges, externalIds); };

    // Drag a dot anywhere inside the graph; a press without movement is a click (pick).
    var dragEnd = 0;
    nodes.forEach(function(n){
      var g = N_[n].g;
      // The click that ends a drag must not also pick the node.
      g.addEventListener("click", function(e){ if(Date.now() - dragEnd < 350) e.stopPropagation(); }, true);
      g.addEventListener("pointerdown", function(ev){
        if(ev.button !== 0) return;
        var rect = svg.getBoundingClientRect(), sc = W / (rect.width || W);
        var moved = false, x0 = ev.clientX, y0 = ev.clientY;
        // Pointer capture retargets the release (and its click) to the dot's group, so
        // the name's own click handler never sees it: decide "explore" here instead.
        var onName = !!(ev.target.closest && ev.target.closest(".node-lbl"));
        g.setPointerCapture(ev.pointerId);
        function mv(e){
          if(!moved && Math.hypot(e.clientX - x0, e.clientY - y0) < 4) return;
          moved = true; g.classList.add("dragging");
          var r = P[n].r;
          P[n].x = Math.max(PAD + r, Math.min(W - PAD - r, (e.clientX - rect.left) * sc));
          P[n].y = Math.max(PAD + r, Math.min(H - PAD - r, (e.clientY - rect.top) * sc));
          layoutNode(n);
          E.forEach(function(x){ if(x.e.from === n || x.e.to === n) layoutEdge(x); });
        }
        function up(){
          g.removeEventListener("pointermove", mv); g.removeEventListener("pointerup", up); g.removeEventListener("pointercancel", up);
          g.classList.remove("dragging");
          if(moved){
            FLOWPOS[n] = [P[n].x / W, P[n].y / H]; saveFlowPos(); syncReset();
            dragEnd = Date.now();
          } else if(onName){
            dragEnd = Date.now();   // swallow the click that follows, so it doesn't also list transactions
            focusOn(n);
          }
        }
        g.addEventListener("pointermove", mv); g.addEventListener("pointerup", up); g.addEventListener("pointercancel", up);
      });
    });
    markPicked();
  }

  // ---------- e-transfers by recipient ----------
  // Parse the counterparty straight out of the description, so e-transfers are
  // grouped by who you paid / who paid you even when no person rule exists.
  function etransferInfo(merchant){
    var u = String(merchant || "").toUpperCase();
    if(u.indexOf("E-TRANSFER") === -1 && u.indexOf("ETRNSFR") === -1 && u.indexOf("E TRANSFER") === -1) return null;
    var cancelled = u.indexOf("CANCELLED") !== -1;
    var m = /(?:E[- ]?TRANSFER|ETRNSFR)\s+(TO|FROM|SENT|RECEIVED|RCVD|RECVD)\b[:\s]*(.*)$/i.exec(merchant);
    var dir = "sent", who = "";
    if(m){ var k = m[1].toUpperCase();
      dir = (k === "FROM" || k === "RECEIVED" || k === "RCVD" || k === "RECVD") ? "received" : "sent";
      who = (m[2] || "").trim(); }
    who = who.replace(/[\s:.\-]+$/, "").trim();
    if(!who) who = "(unspecified)";
    if(cancelled) who = who + " (cancelled)";
    return {dir:dir, who:who};
  }
  function renderEtransfers(rows){
    var el = document.getElementById("etransfers-panel");
    var g = {};
    rows.forEach(function(t){
      if(t.kind !== "transfer") return;
      var info = etransferInfo(t.merchant); if(!info) return;
      var key = info.dir + "|" + info.who;
      var e = g[key] || (g[key] = {dir:info.dir, who:info.who, total:0, count:0, first:t.date, last:t.date});
      e.total += Math.abs(toCents(t.amount)); e.count += 1;
      if(t.date < e.first) e.first = t.date;
      if(t.date > e.last)  e.last = t.date;
    });
    var all = Object.keys(g).map(function(k){ return g[k]; });
    if(!all.length){ el.innerHTML = '<div class="empty">No e-transfers in the current selection.</div>'; return; }
    all.sort(function(a,b){ return b.total - a.total || (a.who < b.who ? -1 : a.who > b.who ? 1 : 0); });
    function tbl(title, dir, who){
      var list = all.filter(function(r){ return r.dir === dir; });
      if(!list.length) return "";
      var sum = list.reduce(function(a,r){ return a + r.total; }, 0);
      return '<h3 style="font-size:14px;margin:14px 0 6px;" class="muted">'+esc(title)+' · '+fmtMoney(sum)+
        ' across '+list.length+' '+(list.length === 1 ? "counterparty" : "counterparties")+'</h3>'+
        '<table><thead><tr><th>'+esc(who)+'</th><th class="num">Total</th><th class="num">Count</th><th>First</th><th>Last</th></tr></thead><tbody>'+
        list.map(function(r){ return '<tr'+pickAttr("et|"+r.dir+"|"+r.who, (r.dir === "sent" ? "E-transfers to " : "E-transfers from ")+r.who, function(t){
          if(t.kind !== "transfer") return false;
          var i = etransferInfo(t.merchant); return !!i && i.dir === r.dir && i.who === r.who; })+'><td>'+esc(r.who)+'</td><td class="num">'+fmtMoney(r.total)+
          '</td><td class="num">'+r.count+'</td><td class="muted">'+esc(r.first)+'</td><td class="muted">'+esc(r.last)+'</td></tr>'; }).join("")+
        '</tbody></table>';
    }
    el.innerHTML = tbl("Sent to", "sent", "Recipient") + tbl("Received from", "received", "Sender");
  }

  // ---------- cash-flow chart ----------
  var chartEl = document.getElementById("cashflow-chart");
  var uplotInst = null;
  var chartDragging = false; // true once a drag begins inside the chart
  // The cash-flow chart builds its own series straight from the filtered
  // transactions at a DYNAMIC granularity, so zooming/filtering to a short window
  // shows finer detail. Granularity is chosen to keep the series at most 365
  // points: daily up to ~1 year, then weekly, then monthly for very long spans —
  // so a short selection shows one point per day, never just a handful.
  var DAY_MS = 86400000;
  // The All / 12mo / YTD buttons constrain the chart's date window (not the
  // global filter), scoped to the currently filtered rows.
  function rangeFilterRows(rows, mode){
    if(mode === "all" || !rows.length) return rows;
    var maxMs = -Infinity;
    rows.forEach(function(t){ var d = dateMs(t.date); if(d != null && d > maxMs) maxMs = d; });
    if(maxMs === -Infinity) return rows;
    var md = new Date(maxMs), cutoff;
    if(mode === "12") cutoff = Date.UTC(md.getUTCFullYear(), md.getUTCMonth() - 11, 1);
    else if(mode === "ytd") cutoff = Date.UTC(md.getUTCFullYear(), 0, 1);
    else return rows;
    return rows.filter(function(t){ var d = dateMs(t.date); return d != null && d >= cutoff; });
  }
  function pickGran(spanDays){
    if(spanDays <= 366) return "day";
    if(spanDays <= 366 * 7) return "week";
    return "month";
  }
  // A fixed Monday epoch anchor for week / bi-week bucketing (1970-01-05 was a Monday).
  var WEEK_ANCHOR = Date.UTC(1970, 0, 5);
  // Bucket-start timestamp in SECONDS (uPlot x is seconds; dateMs returns ms).
  function bucketSec(ms, gran){
    var d = new Date(ms);
    if(gran === "month") return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), 1) / 1000;
    if(gran === "biweek"){
      var nb = Math.floor((ms - WEEK_ANCHOR) / (14 * DAY_MS));
      return (WEEK_ANCHOR + nb * 14 * DAY_MS) / 1000;
    }
    if(gran === "week"){
      var nw = Math.floor((ms - WEEK_ANCHOR) / (7 * DAY_MS));
      return (WEEK_ANCHOR + nw * 7 * DAY_MS) / 1000;
    }
    return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate()) / 1000;
  }
  var GRAN_LABEL = { day: "daily", week: "weekly", biweek: "bi-weekly", month: "monthly" };
  var GRAN_WIN = { day: 30, week: 4, biweek: 3, month: 3 }; // trailing baseline window
  // Compact axis numbers: 50000 -> "50k", -20000 -> "-20k", 1200000 -> "1.2M".
  function fmtAxisNum(v){
    var a = Math.abs(v);
    if(a >= 1e6) return (v / 1e6).toFixed(a % 1e6 ? 1 : 0) + "M";
    if(a >= 1e3) return (v / 1e3).toFixed(a % 1e3 ? 1 : 0) + "k";
    return "" + v;
  }
  // forceGran: "auto" (or falsy) picks granularity from the span; otherwise it is
  // pinned to "week" / "biweek" / "month" regardless of zoom.
  function buildSeries(rows, forceGran){
    var hasFlow = rows.some(isFlow);
    var items = [], minMs = Infinity, maxMs = -Infinity;
    rows.forEach(function(t){
      var include = hasFlow ? isFlow(t) : (t.kind === "transfer" || t.kind === "unknown");
      if(!include) return;
      var ms = dateMs(t.date); if(ms == null) return;
      if(ms < minMs) minMs = ms; if(ms > maxMs) maxMs = ms;
      var p = flowParts(t); items.push([ms, p[0], p[1]]);
    });
    if(!items.length) return { xs:[], inflow:[], outflow:[], net:[], base:[], gran:"day" };
    var gran = (forceGran && forceGran !== "auto")
      ? forceGran : pickGran((maxMs - minMs) / DAY_MS + 1);
    var b = {};
    items.forEach(function(it){ var k = bucketSec(it[0], gran);
      var o = b[k] || (b[k] = { i:0, o:0 }); o.i += it[1]; o.o += it[2]; });
    var keys = Object.keys(b).map(Number).sort(function(a, c){ return a - c; });
    var nets = keys.map(function(k){ return b[k].i - b[k].o; });
    var win = GRAN_WIN[gran] || 3;
    var xs = [], inflow = [], outflow = [], net = [], base = [];
    keys.forEach(function(k, i){ var o = b[k], nv = o.i - o.o;
      xs.push(k); inflow.push(o.i / 100); outflow.push(Math.abs(o.o) / 100); net.push(nv / 100);
      base.push(i > 0 ? mean(nets.slice(Math.max(0, i - win), i)) / 100 : null); });
    return { xs:xs, inflow:inflow, outflow:outflow, net:net, base:base, gran:gran };
  }
  function activeRange(){
    var b = document.querySelector("#range-buttons .active");
    return b ? b.getAttribute("data-range") : "all";
  }
  function activeGran(){
    var b = document.querySelector("#gran-buttons .active");
    return b ? b.getAttribute("data-gran") : "auto";
  }
  // Overlay on the plotted area: (1) life events — periods as shaded bands,
  // points as labelled markers — and (2) auto-tags for the currently-visible
  // unusual-activity flags along the top rail. Shared by the raw chart and the
  // three change-point charts. Runs in device pixels (uPlot's canvas is
  // dpr-scaled), so sizes are *dpr.
  function drawOverlays(u){
    // Events toggled visible; each carries its own colour (e.color) and
    // translucent band fill (e.fill), assigned in allEvents().
    var evs = visibleEvents();
    var anomTags = state.showAnomTags ? anomalyChartTags() : [];
    if(!evs.length && !anomTags.length) return;
    var ctx = u.ctx, dpr = u.pxRatio || window.devicePixelRatio || 1;
    var top = u.bbox.top, bot = u.bbox.top + u.bbox.height;
    var left = u.bbox.left, right = u.bbox.left + u.bbox.width;
    var xmin = u.scales.x.min, xmax = u.scales.x.max;
    function clampX(px){ return px < left ? left : (px > right ? right : px); }
    ctx.save();
    ctx.font = Math.round(11 * dpr) + "px system-ui, -apple-system, sans-serif";
    ctx.textBaseline = "alphabetic";
    // Pass 1 — period bands.
    evs.forEach(function(e){
      if(!isPeriod(e)) return;
      var sMs = dateMs(e.date), eMs = dateMs(e.end); if(sMs == null || eMs == null) return;
      var sTs = sMs / 1000, eTs = eMs / 1000;
      if(eTs < xmin || sTs > xmax) return;                 // entirely off-screen
      var x1 = clampX(u.valToPos(Math.max(sTs, xmin), "x", true));
      var x2 = clampX(u.valToPos(Math.min(eTs, xmax), "x", true));
      if(x2 < x1){ var t = x1; x1 = x2; x2 = t; }
      var col = e.color;
      ctx.fillStyle = e.fill;
      ctx.fillRect(x1, top, Math.max(1, x2 - x1), bot - top);
      // Dashed boundary line at each *true* edge that's actually in view.
      ctx.strokeStyle = col; ctx.lineWidth = dpr; ctx.setLineDash([4 * dpr, 3 * dpr]);
      if(sTs >= xmin){ ctx.beginPath(); ctx.moveTo(x1 + 0.5, top); ctx.lineTo(x1 + 0.5, bot); ctx.stroke(); }
      if(eTs <= xmax){ ctx.beginPath(); ctx.moveTo(x2 - 0.5, top); ctx.lineTo(x2 - 0.5, bot); ctx.stroke(); }
      ctx.setLineDash([]);
      // Label: horizontal inside the band when it's wide enough, clipped to the
      // band; otherwise climbing up the left edge like a point marker.
      ctx.fillStyle = col;
      var label = e.label.length > 44 ? e.label.slice(0, 43) + "…" : e.label;
      if(x2 - x1 >= 60 * dpr){
        ctx.save();
        ctx.beginPath(); ctx.rect(x1, top, x2 - x1, bot - top); ctx.clip();
        ctx.textAlign = "left";
        ctx.fillText(label, x1 + 4 * dpr, top + 13 * dpr);
        ctx.restore();
      } else {
        ctx.save();
        ctx.translate(x1 - 3 * dpr, bot - 4 * dpr); ctx.rotate(-Math.PI / 2);
        ctx.textAlign = "left"; ctx.fillText(label, 0, 0);
        ctx.restore();
      }
    });
    // Pass 2 — point markers.
    evs.forEach(function(e){
      if(isPeriod(e)) return;
      var ms = dateMs(e.date); if(ms == null) return;
      var ts = ms / 1000; if(ts < xmin || ts > xmax) return;
      var x = Math.round(u.valToPos(ts, "x", true));
      var col = e.color;
      ctx.strokeStyle = col; ctx.fillStyle = col;
      ctx.lineWidth = dpr; ctx.setLineDash([4 * dpr, 3 * dpr]);
      ctx.beginPath(); ctx.moveTo(x + 0.5, top); ctx.lineTo(x + 0.5, bot); ctx.stroke();
      // Small flag triangle at the top of the line.
      ctx.setLineDash([]);
      ctx.beginPath(); ctx.moveTo(x, top); ctx.lineTo(x + 7 * dpr, top); ctx.lineTo(x, top + 7 * dpr); ctx.closePath(); ctx.fill();
      // Label climbs upward just left of the line (rotated -90°), clipped to height.
      var label = e.label.length > 28 ? e.label.slice(0, 27) + "…" : e.label;
      ctx.save();
      ctx.translate(x - 3 * dpr, bot - 4 * dpr); ctx.rotate(-Math.PI / 2);
      ctx.textAlign = "left";
      ctx.fillText(label, 0, 0);
      ctx.restore();
    });
    // Pass 3 — auto-tag unusual activity along the top rail: a diamond for a
    // dated flag, a short bar for a month/streak. Coloured by severity. The
    // detail lives in the Unusual-activity table; these are just locators.
    ctx.setLineDash([]);
    var railY = top + 3 * dpr;
    anomTags.forEach(function(g){
      var sTs = g.start / 1000, eTs = (g.end == null ? g.start : g.end) / 1000;
      if(eTs < xmin || sTs > xmax) return;
      ctx.fillStyle = g.color;
      if(g.point){
        var x = clampX(u.valToPos(sTs, "x", true)), r = 3.2 * dpr;
        ctx.beginPath();
        ctx.moveTo(x, railY - r); ctx.lineTo(x + r, railY); ctx.lineTo(x, railY + r); ctx.lineTo(x - r, railY);
        ctx.closePath(); ctx.fill();
      } else {
        var x1 = clampX(u.valToPos(Math.max(sTs, xmin), "x", true));
        var x2 = clampX(u.valToPos(Math.min(eTs, xmax), "x", true));
        if(x2 < x1){ var t = x1; x1 = x2; x2 = t; }
        ctx.fillRect(x1, railY - 2 * dpr, Math.max(2 * dpr, x2 - x1), 4 * dpr);
      }
    });
    ctx.restore();
  }
  // A section popped out into a window gets the window's shape: its main visual
  // fills the leftover height (CSS makes it a flex item) instead of a fixed size.
  function inWin(el){ return !!(el && el.closest && el.closest(".fw-section")); }
  function rawChartHeight(){
    if(!inWin(chartEl)) return 300;
    var legend = uplotInst ? (uplotInst.root.offsetHeight - uplotInst.height) : 34;
    return Math.max(180, chartEl.clientHeight - legend - 4);
  }
  function activeView(){
    var b = document.querySelector("#view-buttons .active");
    return b ? b.getAttribute("data-view") : "raw";
  }
  var cpHeight = 220, cpFitting = false;
  function drawChart(){
    if(!uPlot || !chartEl) return;
    var cpEl = document.getElementById("cp-charts");
    var cpCtl = document.getElementById("cp-controls");
    if(activeView() === "cp" && OPTS.drawChangePoints && cpEl){
      if(uplotInst){ uplotInst.destroy(); uplotInst = null; }
      chartEl.style.display = "none"; cpEl.style.display = ""; if(cpCtl) cpCtl.style.display = "";
      var cpRows = rangeFilterRows(lastRows, activeRange());
      var g = activeGran(), ms = cpRows.map(function(t){ return dateMs(t.date); }).filter(function(x){ return x != null; });
      if(g === "auto"){
        var lo = Math.min.apply(null, ms), hi = Math.max.apply(null, ms);
        g = ms.length ? pickGran((hi - lo) / DAY_MS + 1) : "day";
      }
      var resEl0 = document.getElementById("cashflow-res");
      if(resEl0) resEl0.textContent = GRAN_LABEL[g] + (activeGran() === "auto" ? " (auto)" : "");
      OPTS.drawChangePoints(cpEl, {
        rows: cpRows, gran: g, dateMs: dateMs, isFlow: isFlow, flowParts: flowParts,
        drawOverlays: drawOverlays, fmtAxisNum: fmtAxisNum, fmtMoney: fmtMoney,
        dark: document.documentElement.getAttribute("data-theme") === "dark",
        onDragRange: applyChartRange,
        chartHeight: cpHeight,
        onPickBucket: function(metric, startIso, days){
          var st = dateMs(startIso); if(st == null) return;
          pickBucket(st, st + days * DAY_MS, g, metric);
        }
      });
      document.getElementById("cashflow-empty").style.display = cpRows.length ? "none" : "block";
      // In a window: grow/shrink the three charts so they fill the height. Measured
      // a frame later (after layout); one re-fit pass, guarded against loops.
      if(inWin(cpEl)){
        if(!cpFitting) requestAnimationFrame(function(){
          if(!inWin(cpEl) || activeView() !== "cp") return;
          var spare = cpEl.clientHeight - cpEl.scrollHeight;
          var want = Math.max(150, Math.round(cpHeight + spare / 3));
          if(Math.abs(want - cpHeight) > 6){ cpHeight = want; cpFitting = true; try { drawChart(); } finally { cpFitting = false; } }
        });
      } else cpHeight = 220;
      return;
    }
    chartEl.style.display = ""; if(cpEl) cpEl.style.display = "none"; if(cpCtl) cpCtl.style.display = "none";
    var s = buildSeries(rangeFilterRows(lastRows, activeRange()), activeGran());
    var xs = s.xs, inflow = s.inflow, outflow = s.outflow, net = s.net, base = s.base;
    var resEl = document.getElementById("cashflow-res");
    if(resEl) resEl.textContent = xs.length
      ? (GRAN_LABEL[s.gran] + (activeGran() === "auto" ? " (auto)" : "") + " · " + xs.length + " pts")
      : "";
    if(uplotInst){ uplotInst.destroy(); uplotInst = null; }
    var empty = document.getElementById("cashflow-empty");
    if(!xs.length){ if(empty) empty.style.display = "block"; return; }
    if(empty) empty.style.display = "none";
    var dark = document.documentElement.getAttribute("data-theme") === "dark";
    var stroke = dark ? "#9aa3b2" : "#6b7280";
    uplotInst = new uPlot({
      width: chartEl.clientWidth || 640, height: rawChartHeight(),
      legend: { live: true },
      // Compact y labels (50k, -20k, 1.2M) so big amounts don't overflow the gutter;
      // size reserves enough room for the widest label.
      axes: [ { stroke: stroke },
              { stroke: stroke, size: 58,
                values: function(u, vals){ return vals.map(fmtAxisNum); } } ],
      scales: { x: { time: true } },
      // Drag selects a date range to filter by (instead of zooming the axis):
      // setScale:false keeps the selection box, and the mouseup hook below turns
      // it into the From/To month filter so every section follows.
      cursor: { drag: { x: true, y: false, setScale: false } },
      // Mark that a drag started in the chart; the release is handled by a
      // document-level mouseup (below) so it still applies if the pointer left
      // the chart's bounds before release. (u.over dies with each instance, so
      // this listener is auto-cleaned on redraw — no leak.)
      hooks: {
        ready: [function(u){
          u.over.addEventListener("mousedown", function(){ chartDragging = true; });
          // A click (not a drag) on a bucket lists its transactions; the series
          // nearest the pointer (In / Out / Net) decides which side.
          u.over.addEventListener("click", function(){
            if(u.select && u.select.width > 8) return;
            var i = u.cursor.idx; if(i == null || u.data[0][i] == null) return;
            var best = 3, bd = Infinity;
            [1, 2, 3].forEach(function(si){
              var v = u.data[si][i]; if(v == null) return;
              var dd = Math.abs(u.valToPos(v, "y") - u.cursor.top);
              if(dd < bd){ bd = dd; best = si; }
            });
            var st = u.data[0][i] * 1000;
            pickBucket(st, bucketEndMs(st, s.gran), s.gran, best === 1 ? "in" : best === 2 ? "out" : "net");
          });
        }],
        // Overlay on the plotted area: (1) life events — periods as shaded bands,
        // points as labelled markers — and (2) auto-tags for the currently-visible
        // unusual-activity flags along the top rail. Runs in device pixels (uPlot's
        // canvas is dpr-scaled), so sizes are *dpr.
        draw: [drawOverlays]
      },
      series: [ {},
        { label: "In",  stroke: "#10915a", width: 2 },
        { label: "Out", stroke: "#c2384b", width: 2 },
        { label: "Net", stroke: "#3b6ea5", width: 2 },
        { label: "Baseline", stroke: "#b8862c", width: 1, dash: [6,4] } ]
    }, [xs, inflow, outflow, net, base], chartEl);
    if(inWin(chartEl)){ var hFit = rawChartHeight(); if(Math.abs(hFit - uplotInst.height) > 2) uplotInst.setSize({ width: chartEl.clientWidth || 640, height: hFit }); }
  }
  // Map a uPlot x-value (seconds) back to a "YYYY-MM" month key.
  function tsToMonth(ts){
    var d = new Date(ts * 1000);
    return d.getUTCFullYear() + "-" + ("0" + (d.getUTCMonth() + 1)).slice(-2);
  }
  // Turn a dragged [minTs,maxTs] selection into the From/To month filter.
  function applyChartRange(a, b){
    var minTs = Math.min(a, b), maxTs = Math.max(a, b);
    // Prefer the actual month buckets whose data point falls inside the drag;
    // fall back to the calendar month at each edge (clamped to known months).
    var inRange = MONTHS.filter(function(mo){
      var ts = monthTs(mo); return ts !== null && ts >= minTs && ts <= maxTs; });
    var from, to;
    if(inRange.length){ from = inRange[0]; to = inRange[inRange.length - 1]; }
    else {
      from = tsToMonth(minTs); to = tsToMonth(maxTs);
      if(MONTHS.length){
        if(from < MONTHS[0]) from = MONTHS[0];
        if(to > MONTHS[MONTHS.length - 1]) to = MONTHS[MONTHS.length - 1];
      }
    }
    state.monthFrom = from; state.monthTo = to;
    if(SEL && SEL.time) SEL = null; // a dot pick is stale once the timeframe changes
    buildFilterBar(); rerender();
  }

  // ---------- filter UI ----------
  function facetBtnLabel(def){
    var f = state[def.key]; var parts = [];
    if(f.inc.size) parts.push('<span class="cnt pos">' + f.inc.size + ' ✓</span>');
    if(f.exc.size) parts.push('<span class="cnt neg">' + f.exc.size + ' ≠</span>');
    return def.label + ' ' + (parts.length ? '<span class="cnt">(</span>' + parts.join('<span class="cnt"> · </span>') + '<span class="cnt">)</span>'
                                            : '<span class="cnt muted">· all</span>');
  }
  function buildFilterBar(){
    var bar = document.getElementById("filterbar");
    bar.innerHTML = "";
    var lab = document.createElement("span"); lab.className = "fb-label"; lab.textContent = "Filter:";
    bar.appendChild(lab);

    FACET_DEFS.forEach(function(def){
      var values = FACETS[def.key]; if(!values.length) return;
      var wrap = document.createElement("div"); wrap.className = "facet"; wrap.setAttribute("data-facet", def.key);
      var btn = document.createElement("button"); btn.type = "button"; btn.className = "btn facet-btn";
      btn.innerHTML = facetBtnLabel(def);
      var menu = document.createElement("div"); menu.className = "facet-menu";
      var f = state[def.key];
      // Header: legend for the two columns (keep ✓ / drop ≠) plus a clear shortcut.
      var head = document.createElement("div"); head.className = "facet-head";
      head.innerHTML = '<span class="col-h"><span class="pos">✓</span> keep · <span class="neg">≠</span> drop</span>';
      var clear = document.createElement("a"); clear.textContent = "clear"; clear.className = "facet-clear";
      head.appendChild(clear); menu.appendChild(head);

      var rows = [];
      values.forEach(function(v){
        var l = document.createElement("label"); l.className = "facet-row";
        var inc = document.createElement("input"); inc.type = "checkbox"; inc.className = "f-inc"; inc.title = "keep only this";
        var exc = document.createElement("input"); exc.type = "checkbox"; exc.className = "f-exc"; exc.title = "exclude this";
        inc.checked = f.inc.has(v); exc.checked = f.exc.has(v);
        // Include and exclude are mutually exclusive per value; ticking one clears the other.
        inc.addEventListener("change", function(){
          if(inc.checked){ f.inc.add(v); f.exc.delete(v); exc.checked = false; } else { f.inc.delete(v); }
          btn.innerHTML = facetBtnLabel(def); rerender();
        });
        exc.addEventListener("change", function(){
          if(exc.checked){ f.exc.add(v); f.inc.delete(v); inc.checked = false; } else { f.exc.delete(v); }
          btn.innerHTML = facetBtnLabel(def); rerender();
        });
        var sp = document.createElement("span"); sp.className = "facet-val"; sp.textContent = v;
        l.appendChild(inc); l.appendChild(exc); l.appendChild(sp); menu.appendChild(l);
        rows.push([inc, exc]);
      });
      clear.addEventListener("click", function(){ f.inc.clear(); f.exc.clear();
        rows.forEach(function(r){ r[0].checked = false; r[1].checked = false; });
        btn.innerHTML = facetBtnLabel(def); rerender(); });
      btn.addEventListener("click", function(ev){ ev.stopPropagation();
        var open = wrap.classList.contains("open");
        closeMenus(); if(!open) wrap.classList.add("open"); });
      wrap.appendChild(btn); wrap.appendChild(menu); bar.appendChild(wrap);
    });

    // month range
    function monthSelect(id, label, def){
      var l = document.createElement("label"); l.className = "fb-label"; l.style.textTransform = "none";
      l.style.letterSpacing = "0"; l.textContent = label + " ";
      var s = document.createElement("select"); s.id = id;
      var any = document.createElement("option"); any.value = ""; any.textContent = "—"; s.appendChild(any);
      MONTHS.forEach(function(mo){ var o = document.createElement("option"); o.value = mo; o.textContent = mo; s.appendChild(o); });
      s.value = def || "";
      s.addEventListener("change", function(){
        state.monthFrom = document.getElementById("month-from").value || null;
        state.monthTo = document.getElementById("month-to").value || null;
        rerender();
      });
      l.appendChild(s); return l;
    }
    // Inline "≠" toggle that flips a range from keep-only to exclude.
    function negToggle(getter, setter, title){
      var l = document.createElement("label");
      l.style.cssText = "display:inline-flex;gap:3px;align-items:center;font-size:12px;color:var(--muted);cursor:pointer;";
      l.title = title;
      var cb = document.createElement("input"); cb.type = "checkbox"; cb.checked = !!getter();
      cb.addEventListener("change", function(){ setter(cb.checked); rerender(); });
      var sp = document.createElement("span"); sp.textContent = "≠";
      l.appendChild(cb); l.appendChild(sp); return l;
    }

    bar.appendChild(monthSelect("month-from", "From"));
    bar.appendChild(monthSelect("month-to", "To"));
    bar.appendChild(negToggle(function(){ return state.negMonths; },
      function(v){ state.negMonths = v; },
      "Exclude this date range instead of keeping only it"));

    // Amount range (compares the transaction's magnitude, in dollars).
    function amtInput(id, ph, centsVal){
      var i = document.createElement("input"); i.type = "number"; i.id = id; i.min = "0"; i.step = "0.01";
      i.placeholder = ph; i.style.width = "84px";
      if(centsVal != null) i.value = (centsVal / 100).toString();
      var t;
      i.addEventListener("input", function(){
        clearTimeout(t); t = setTimeout(function(){
          state.amtMin = document.getElementById("amt-min").value === "" ? null : toCents(document.getElementById("amt-min").value);
          state.amtMax = document.getElementById("amt-max").value === "" ? null : toCents(document.getElementById("amt-max").value);
          rerender();
        }, 200);
      });
      return i;
    }
    var amtLabel = document.createElement("span"); amtLabel.className = "fb-label"; amtLabel.textContent = "Amount $";
    bar.appendChild(amtLabel);
    bar.appendChild(amtInput("amt-min", "min", state.amtMin));
    bar.appendChild(amtInput("amt-max", "max", state.amtMax));
    bar.appendChild(negToggle(function(){ return state.negAmt; },
      function(v){ state.negAmt = v; },
      "Exclude amounts in this range instead of keeping only them"));

    var search = document.createElement("input"); search.type = "text"; search.id = "search";
    search.placeholder = "merchant regex…"; search.value = state.search; search.title = "Case-insensitive regular expression matched against the merchant";
    var st;
    search.addEventListener("input", function(){
      clearTimeout(st); st = setTimeout(function(){
        state.search = search.value.trim();
        compileSearch();
        search.style.borderColor = SEARCH_BAD ? "var(--neg)" : "var(--line)";
        rerender();
      }, 200);
    });
    bar.appendChild(search);

    var searchEx = document.createElement("input"); searchEx.type = "text"; searchEx.id = "search-excl";
    searchEx.placeholder = "exclude merchant regex…"; searchEx.value = state.searchExclude;
    searchEx.title = "Case-insensitive regex; transactions whose merchant MATCHES are excluded (e.g. ABM)";
    var stx;
    searchEx.addEventListener("input", function(){
      clearTimeout(stx); stx = setTimeout(function(){
        state.searchExclude = searchEx.value.trim();
        compileSearch();
        searchEx.style.borderColor = EXCL_BAD ? "var(--neg)" : "var(--line)";
        rerender();
      }, 200);
    });
    bar.appendChild(searchEx);

    var reset = document.createElement("button"); reset.type = "button"; reset.className = "btn"; reset.textContent = "Reset";
    reset.addEventListener("click", function(){
      state.persons = freshFacet(); state.accounts = freshFacet(); state.categories = freshFacet();
      state.subcategories = freshFacet(); state.kinds = freshFacet();
      state.monthFrom = null; state.monthTo = null; state.search = ""; state.searchExclude = "";
      state.amtMin = null; state.amtMax = null;
      state.negMonths = false; state.negAmt = false;
      buildFilterBar(); rerender();
    });
    bar.appendChild(reset);
  }
  function closeMenus(){ document.querySelectorAll(".facet.open").forEach(function(f){ f.classList.remove("open"); }); }
  document.addEventListener("click", function(){
    closeMenus();
    document.querySelectorAll(".flow .edge-g.sel").forEach(function(x){ x.classList.remove("sel"); });
  });

  function renderActiveBar(filteredLen){
    var el = document.getElementById("activebar"); el.innerHTML = "";
    function pill(label, key, val, neg){
      var p = document.createElement("span"); p.className = "pill" + (neg ? " pill-neg" : "");
      p.innerHTML = '<b>'+esc(label)+':</b> '+esc(val)+' <span class="x">×</span>';
      p.querySelector(".x").addEventListener("click", key);
      el.appendChild(p);
    }
    FACET_DEFS.forEach(function(def){
      var f = state[def.key];
      // Keep (✓) and drop (≠) render as separate pills so both can show at once.
      if(f.inc.size)
        pill(def.label, function(){ f.inc.clear(); buildFilterBar(); rerender(); }, Array.from(f.inc).join(", "), false);
      if(f.exc.size)
        pill(def.label, function(){ f.exc.clear(); buildFilterBar(); rerender(); }, "≠ " + Array.from(f.exc).join(", "), true);
    });
    if(state.monthFrom || state.monthTo)
      pill("Months", function(){ state.monthFrom = null; state.monthTo = null; state.negMonths = false; buildFilterBar(); rerender(); },
        (state.negMonths ? "≠ " : "") + (state.monthFrom||"…") + " → " + (state.monthTo||"…"), state.negMonths);
    if(state.amtMin != null || state.amtMax != null)
      pill("Amount", function(){ state.amtMin = null; state.amtMax = null; state.negAmt = false; buildFilterBar(); rerender(); },
        (state.negAmt ? "≠ " : "") + "$" + (state.amtMin != null ? (state.amtMin/100) : "…") + " – $" + (state.amtMax != null ? (state.amtMax/100) : "…"), state.negAmt);
    if(state.search)
      pill("Merchant /re/", function(){ state.search = ""; buildFilterBar(); rerender(); },
        state.search + (SEARCH_BAD ? " (invalid — using substring)" : ""));
    if(anyFilterActive()){
      var note = document.createElement("span"); note.className = "scope-note";
      note.textContent = filteredLen + " of " + TX.length + " transactions";
      el.appendChild(note);
    }
  }

  // ---------- life-events editor ----------
  // Rendered once (independent of filters); the chart markers refresh on every
  // drawChart() via the draw hook. Add/remove persists user events to localStorage.
  function renderEvents(){
    var host = document.getElementById("events-ui"); if(!host) return;
    host.innerHTML = "";
    var hdr = document.createElement("div"); hdr.className = "events-hdr";
    hdr.innerHTML = '<span class="events-title">Life events</span>' +
      '<span class="muted" style="font-size:12px;">markers (and shaded spans for periods) on the chart — see how milestones line up with your money</span>';
    var addBtn = document.createElement("button"); addBtn.type = "button"; addBtn.className = "btn"; addBtn.textContent = "+ Add event";
    hdr.appendChild(addBtn); host.appendChild(hdr);

    // Toggle: auto-tag the currently-visible unusual-activity flags on the chart.
    var tagWrap = document.createElement("label"); tagWrap.className = "anomtag-toggle";
    var tagCb = document.createElement("input"); tagCb.type = "checkbox"; tagCb.checked = !!state.showAnomTags;
    tagWrap.appendChild(tagCb);
    tagWrap.appendChild(document.createTextNode(" Tag unusual activity on the chart"));
    tagWrap.appendChild((function(){ var s = document.createElement("span"); s.className = "anomtag-key";
      s.innerHTML = ' <span class="ak ak-h"></span><span class="ak ak-m"></span><span class="ak ak-l"></span> by severity'; return s; })());
    tagCb.addEventListener("change", function(){
      state.showAnomTags = tagCb.checked;
      try { window.localStorage.setItem(ANOMTAGS_KEY, tagCb.checked ? "1" : "0"); } catch(e){}
      drawChart();
    });
    host.appendChild(tagWrap);

    var form = document.createElement("div"); form.className = "events-form"; form.style.display = "none";
    var dIn = document.createElement("input"); dIn.type = "date"; dIn.title = "Date (start of a period)";
    var toSep = document.createElement("span"); toSep.className = "muted"; toSep.style.fontSize = "12px"; toSep.textContent = "to";
    var eIn = document.createElement("input"); eIn.type = "date"; eIn.title = "End date — optional; set it to mark a period (shaded band)";
    var lIn = document.createElement("input"); lIn.type = "text"; lIn.placeholder = "label (e.g. New job)"; lIn.maxLength = 60;
    var nIn = document.createElement("input"); nIn.type = "text"; nIn.placeholder = "note (optional)"; nIn.maxLength = 140;
    var save = document.createElement("button"); save.type = "button"; save.className = "btn"; save.textContent = "Add";
    var err = document.createElement("span"); err.className = "events-err";
    form.appendChild(dIn); form.appendChild(toSep); form.appendChild(eIn);
    form.appendChild(lIn); form.appendChild(nIn); form.appendChild(save); form.appendChild(err);
    host.appendChild(form);
    addBtn.addEventListener("click", function(){
      form.style.display = form.style.display === "none" ? "flex" : "none";
      if(form.style.display !== "none") dIn.focus();
    });
    function doAdd(){
      var date = dIn.value, end = eIn.value, label = (lIn.value || "").trim();
      if(!isIsoDate(date)){ err.textContent = "pick a date"; return; }
      if(end){
        if(!isIsoDate(end)){ err.textContent = "end date is invalid"; return; }
        if(end < date){ err.textContent = "end must be on/after the start date"; return; }
      }
      if(!label){ err.textContent = "enter a label"; return; }
      var list = userEvents();
      list.push({ date:date, end:end || "", label:label, note:(nIn.value || "").trim() });
      if(!saveUserEvents(list)){ err.textContent = "couldn't save — browser storage is blocked"; return; }
      eIn.value = ""; lIn.value = ""; nIn.value = ""; err.textContent = "";
      renderEvents(); drawChart();
    }
    save.addEventListener("click", doAdd);
    lIn.addEventListener("keydown", function(e){ if(e.key === "Enter") doAdd(); });
    nIn.addEventListener("keydown", function(e){ if(e.key === "Enter") doAdd(); });

    var evs = allEvents();
    if(!evs.length){
      var none = document.createElement("div"); none.className = "muted"; none.style.fontSize = "12px"; none.style.marginTop = "6px";
      none.textContent = "No events yet — add one above (saved in this browser), or list them in events.toml to bake them into the report.";
      host.appendChild(none); return;
    }
    var hint = document.createElement("div"); hint.className = "muted"; hint.style.fontSize = "11px"; hint.style.margin = "2px 0 6px";
    hint.textContent = "Each event has its own colour. Click an event to hide/show it on the chart.";
    host.appendChild(hint);
    var list = document.createElement("div"); list.className = "events-list";
    evs.forEach(function(e){
      var period = isPeriod(e);
      var chip = document.createElement("span");
      chip.className = "event-chip " + (e.source === "user" ? "ev-user" : "ev-config") + (e.hidden ? " ev-hidden" : "");
      chip.title = e.hidden ? "Hidden — click to show on chart" : "Shown on chart — click to hide";
      var when = period ? esc(e.date) + ' → ' + esc(e.end) : esc(e.date);
      // The dot carries this event's unique colour (matches its chart band/marker).
      chip.innerHTML = '<span class="ev-dot' + (period ? ' ev-span' : '') + '" style="background:' + e.color + '"></span><b>' + when + '</b> ' + esc(e.label) +
        (e.note ? ' <span class="muted">— ' + esc(e.note) + '</span>' : '');
      // Click anywhere on the chip toggles its visibility on the chart.
      chip.addEventListener("click", function(){ toggleEventHidden(e); renderEvents(); drawChart(); });
      if(e.source === "user"){
        var x = document.createElement("span"); x.className = "x"; x.textContent = "×"; x.title = "remove";
        x.addEventListener("click", function(ev){
          ev.stopPropagation(); // don't also toggle visibility
          var rest = userEvents().filter(function(u){
            return !(u.date === e.date && (u.end || "") === (e.end || "") && u.label === e.label && (u.note || "") === (e.note || "")); });
          saveUserEvents(rest); renderEvents(); drawChart();
        });
        chip.appendChild(x);
      } else {
        var tag = document.createElement("span"); tag.className = "ev-tag"; tag.textContent = "shared"; tag.title = "from events.toml";
        chip.appendChild(tag);
      }
      list.appendChild(chip);
    });
    host.appendChild(list);
  }

  // ---------- orchestration ----------
  var lastRows = [];
  function rerender(){
    compileSearch();
    var rows = TX.filter(passes);
    lastRows = rows;
    var f = computeFlows(rows);
    renderCards(f);
    renderSpend(f, rows);
    renderIncome(f, rows);
    renderTransfers(rows);
    renderEtransfers(rows);
    renderRecurring(rows);
    renderAnomalies();
    renderTrends(rows);
    renderByEntity("by-person-panel", "person", "Person", rows);
    renderByEntity("by-account-panel", "account", "Account", rows);
    renderMerchants(rows);
    renderTxWin();
    markPicked();
    renderActiveBar(rows.length);
    drawChart();
  }

  // Make each data section (h2.sec-h) collapse its panel; closed by default.
  function initSections(){
    document.querySelectorAll("h2.sec-h").forEach(function(h){
      h.insertAdjacentHTML("afterbegin", '<span class="caret">▾</span> ');
      var panel = h.nextElementSibling;
      h.classList.add("closed");
      if(panel) panel.classList.add("sec-closed");
      h.addEventListener("click", function(ev){
        // Clicking the help icon shows its tooltip; don't toggle the section.
        if(ev.target.closest(".help")) return;
        // While the section lives in a window, its heading only brings that window up.
        if(h.classList.contains("yanked")) return;
        h.classList.toggle("closed");
        if(panel) panel.classList.toggle("sec-closed");
      });
    });
  }

  buildFilterBar();
  initSections();
  renderEvents();
  rerender();

  var rb = document.getElementById("range-buttons");
  if(rb){ rb.addEventListener("click", function(ev){
    var b = ev.target.closest("button[data-range]"); if(!b) return;
    rb.querySelectorAll("button").forEach(function(x){ x.classList.remove("active"); });
    b.classList.add("active"); drawChart();
  }); }

  // Bucketing mode: Auto (granularity from the zoom) / Weekly / Bi-weekly / Monthly.
  var gb = document.getElementById("gran-buttons");
  if(gb){ gb.addEventListener("click", function(ev){
    var b = ev.target.closest("button[data-gran]"); if(!b) return;
    gb.querySelectorAll("button").forEach(function(x){ x.classList.remove("active"); });
    b.classList.add("active"); drawChart();
  }); }

  // A chart drag can end outside the plot; uPlot tracks the release on the
  // document, so we do too. Bound once (never leaks) and reads the live instance.
  document.addEventListener("mouseup", function(){
    if(!chartDragging) return;
    chartDragging = false;
    var u = uplotInst;
    if(u && u.select && u.select.width > 8){
      applyChartRange(u.posToVal(u.select.left, "x"),
                      u.posToVal(u.select.left + u.select.width, "x"));
    }
  });

  // Raw buckets vs change-point (level) view.
  var vb = document.getElementById("view-buttons");
  if(vb){ vb.addEventListener("click", function(ev){
    var b = ev.target.closest("button[data-view]"); if(!b) return;
    vb.querySelectorAll("button").forEach(function(x){ x.classList.remove("active"); });
    b.classList.add("active");
    try { window.localStorage.setItem("wdmg.view.v1", b.getAttribute("data-view")); } catch(e){}
    drawChart();
  });
    try {
      var savedView = window.localStorage.getItem("wdmg.view.v1");
      var sb = savedView && vb.querySelector('button[data-view="' + savedView + '"]');
      if(sb){ vb.querySelectorAll("button").forEach(function(x){ x.classList.remove("active"); }); sb.classList.add("active"); drawChart(); }
    } catch(e){}
  }

  var resizeTimer = null;
  window.addEventListener("resize", function(){
    if(activeView() === "cp"){ clearTimeout(resizeTimer); resizeTimer = setTimeout(drawChart, 150); }
    if(uplotInst && chartEl){ uplotInst.setSize({ width: chartEl.clientWidth||640, height: rawChartHeight() }); }
    renderTransfers(lastRows);
  });

  var tt = document.getElementById("theme-toggle");
  if(tt){ tt.addEventListener("click", function(){
    var cur = document.documentElement.getAttribute("data-theme");
    document.documentElement.setAttribute("data-theme", cur === "dark" ? "light" : "dark");
    drawChart();
  }); }
  return { redraw: drawChart };
}
