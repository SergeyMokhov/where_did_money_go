import { describe, expect, it } from 'vitest';
import { commonPrefix, describeFilename, filenameRegex, parseFilenameRegex, propose, type FileInfo } from './matchguide';

const f = (name: string, profile: string | null, fileId: string | null = null): FileInfo => ({ name, profile, fileId });

describe('commonPrefix', () => {
  it('drops download counters, extensions, dates and generic words', () => {
    expect(commonPrefix(['Card 7012.CSV', 'Card 7012(2).CSV', 'Card 7012 (11).CSV'])).toBe('Card 7012');
    expect(commonPrefix(['statement_2026-04-06.csv', 'statement_2026-01-06.csv'])).toBe('statement');
    expect(commonPrefix(['Chq_Main_all.CSV'])).toBe('Chq_Main');
    expect(commonPrefix(['download.csv'])).toBe('download');
  });
  it('never stops mid-word and gives up on a too-short start', () => {
    expect(commonPrefix(['savings_jan.csv', 'savory_feb.csv'])).toBe('');
    expect(commonPrefix(['ab.csv', 'xy.csv'])).toBe('');
  });
});

describe('filename regex round trip', () => {
  it('reads back what it writes', () => {
    for (const [mode, value] of [['starts', 'Card 7012 (x)'], ['contains', 'a.b+c']] as const) {
      const re = filenameRegex(mode, value, [])!;
      expect(parseFilenameRegex(re)).toMatchObject({ mode, value });
    }
    const re = filenameRegex('exact', '', ['a|b.csv', 'c.csv'])!;
    expect(parseFilenameRegex(re)).toMatchObject({ mode: 'exact', names: ['a|b.csv', 'c.csv'] });
    expect(parseFilenameRegex('(?i)^Chq_all\\.CSV$')).toMatchObject({ mode: 'exact', names: ['Chq_all.CSV'] });
    expect(parseFilenameRegex('(?i)(debit|3344)').mode).toBe('pattern');
    expect(describeFilename('(?i)^Card 7012')).toBe('file name starts with “Card 7012”');
  });
});

describe('propose', () => {
  const files = [
    f('Card 7012.CSV', 'rewards_credit', '7012'), f('Card 7012(2).CSV', 'rewards_credit', '7012'),
    f('Card 4444.CSV', 'rewards_credit', '4444'),
    f('Chq_Main_all.CSV', 'bank_ledger'), f('Savings_all.CSV', 'bank_ledger'),
    f('download.csv', 'split_description_ledger'),
  ];
  it('prefers the number the format reads from the file name', () => {
    const c = propose(files, new Set(['Card 7012.CSV', 'Card 7012(2).CSV']));
    expect(c).toMatchObject({ profile: 'rewards_credit', fileId: '7012', nameMode: 'none' });
  });
  it('prefers the name over a leading number shared with other cards', () => {
    const fs = [f('4111 xxxx xxxx 7012.CSV', 'rewards_credit', '4111'), f('4111 xxxx xxxx 7012(2).CSV', 'rewards_credit', '4111'),
      f('4111 xxxx xxxx 1111.CSV', 'rewards_credit', '4111')];
    const c = propose(fs, new Set(['4111 xxxx xxxx 7012.CSV', '4111 xxxx xxxx 7012(2).CSV']));
    expect(c).toMatchObject({ fileId: null, nameMode: 'starts', nameValue: '4111 xxxx xxxx 7012' });
  });
  it('uses the start of the name when the format is shared', () => {
    const c = propose(files, new Set(['Chq_Main_all.CSV']));
    expect(c).toMatchObject({ profile: 'bank_ledger', nameMode: 'starts', nameValue: 'Chq_Main' });
  });
  it('keeps the format with the name, and falls back to exact names', () => {
    expect(propose(files, new Set(['download.csv']))).toMatchObject({ profile: 'split_description_ledger', nameMode: 'starts', nameValue: 'download' });
    const mixed = propose(files, new Set(['Card 4444.CSV', 'Savings_all.CSV']));
    expect(mixed).toMatchObject({ profile: null, nameMode: 'exact' });
  });
});
