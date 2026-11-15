// Thin typed wrapper over the Rust engine compiled to WebAssembly
// (`scripts/build-wasm.sh` generates ./wasm). The .wasm is inlined into the
// single-file build as a data: URL, so this works from file:// with no network.
import init, * as wasm from './wasm/wdmg.js';
import wasmUrl from './wasm/wdmg_bg.wasm?url';
import { CONFIG_NAMES, type ProjectDoc } from './store';

let ready: Promise<unknown> | null = null;
export function initEngine(): Promise<unknown> {
  if (!ready) ready = init({ module_or_path: wasmUrl });
  return ready;
}

export interface LoadReport {
  loaded: { file: string; profile: string; rows: number }[];
  skipped: string[];
  errors: { file: string; error: string }[];
}

export interface AnalyzeResult {
  analysis: any;
  review_queue: any;
  load_report: LoadReport;
}

/** A project's config files as {name: toml}; missing names fall back to defaults in the engine. */
export function projectConfig(doc: ProjectDoc): Record<string, string> {
  const dec = new TextDecoder();
  const config: Record<string, string> = {};
  for (const f of doc.files) {
    if (f.role === 'config' && (CONFIG_NAMES as readonly string[]).includes(f.name)) {
      config[f.name] = dec.decode(f.bytes);
    }
  }
  return config;
}

const dataFiles = (doc: ProjectDoc) =>
  doc.files.filter((f) => f.role === 'data').map((f) => ({ name: f.name, bytes: f.bytes }));

/** Run the full Rust pipeline over a project's files. Throws with the engine's message. */
export function analyzeProject(doc: ProjectDoc): AnalyzeResult {
  return JSON.parse(wasm.analyze(dataFiles(doc), JSON.stringify(projectConfig(doc))));
}

export interface FileCheck {
  file: string; profile: string | null; rows: number; error: string | null; unrecognized: boolean;
  sample: { date: string; amount: string; merchant: string; kind: string }[];
  /** Rows per account after identity resolution; `unassigned` = no account claims them. */
  accounts: Record<string, number>;
  /** Rows that also fit another account, shadowed by the one they went to. */
  also: Record<string, number>;
  /** Rows per cardholder value (empty when the format has no cardholder column). */
  cardholders: Record<string, number>;
  /** Id the format captures from the file name (e.g. a card's last 4). */
  file_id: string | null;
}

/** Detect + parse each file on its own (cheap; no analysis). */
export function checkFiles(files: { name: string; bytes: Uint8Array }[], config: Record<string, string>): FileCheck[] {
  return JSON.parse(wasm.check_files(files, JSON.stringify(config)));
}
export const checkProject = (doc: ProjectDoc) => checkFiles(dataFiles(doc), projectConfig(doc));

/** The engine's embedded default config, as {filename: toml}. */
export function defaultConfig(): Record<string, string> {
  return JSON.parse(wasm.default_config());
}

export type Metric = 'net' | 'in' | 'out';
export type Sensitivity = 'standard' | 'sensitive';
export type Gran = 'day' | 'week' | 'biweek' | 'month';

export interface Interval { k: number; n: number; lo: number; hi: number }
export interface Segment {
  start: string; end: string; days: number;
  level_per_day: number; ci95: [number, number] | null;
  unusual_buckets: Interval; neg_weeks: Interval | null;
}
export interface Change { date: string; delta_per_day: number; good: boolean }
export interface Bucket {
  start: string; days: number; value: number; expected: number;
  lo: number | null; hi: number | null; partial: boolean; unusual: 'high' | 'low' | null; segment: number;
}
export interface CpResult {
  block_days: number; block_kind: 'fixed' | 'calendar_month'; sigma: number;
  segments: Segment[]; changes: Change[]; buckets: Bucket[];
  tentative: null | { start: string; days: number; level_per_day: number; delta_per_day: number; good: boolean };
  trend: null | { since: string; level_per_day: number; slope: number; slope_ci95: [number, number]; direction: string };
}

export function changepoints(input: {
  start: string; daily: number[]; metric: Metric; sensitivity: Sensitivity; min_days: number; bucket: Gran;
}): CpResult {
  return JSON.parse(wasm.changepoints(JSON.stringify(input)));
}

// ---------- people.toml (household identity) ----------
export interface PeopleAccount {
  id: string; kind: string; external: boolean;
  cardholder?: string | null; card_last4?: string | null; filename?: string | null; profile?: string | null;
}
export interface PeoplePerson { id: string; display_name: string; account: PeopleAccount[] }
export interface PeopleDoc {
  schema_version: number; person: PeoplePerson[];
  transfer_rule: { pattern: string; to: string; priority: number }[];
}
export type PeopleOp =
  | { op: 'add_person'; id: string; display_name?: string }
  | { op: 'rename_person'; from: string; to: string }
  | { op: 'rename_account'; from: string; to: string }
  | { op: 'move_account'; account: string; to_person: string }
  | { op: 'add_account'; person: string; id: string; kind: string; file?: string; external?: boolean }
  | { op: 'link_file'; account: string; file: string }
  | { op: 'set_kind'; account: string; kind: string }
  | { op: 'set_match'; account: string; filename?: string | null; profile?: string | null; cardholder?: string | null; card_last4?: string | null }
  | { op: 'make_external'; account: string }
  | { op: 'delete_account'; account: string }
  | { op: 'delete_person'; person: string };

/** The project's people.toml, or the built-in default when it has none. */
export function peopleText(doc: ProjectDoc): string {
  return projectConfig(doc)['people.toml'] ?? defaultConfig()['people.toml'] ?? 'schema_version = 1\n';
}
export function parsePeople(text: string): PeopleDoc {
  return JSON.parse(wasm.parse_people(text));
}
/** Apply edits with the engine (comments and layout in the file are kept). Throws on a bad edit. */
export function editPeople(text: string, ops: PeopleOp[]): string {
  return wasm.edit_people(text, JSON.stringify(ops));
}
