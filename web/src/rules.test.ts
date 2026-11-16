import { describe, expect, it } from 'vitest';
import { boilerplateWords, lcs, nearMisses, rangeFromTicks, describePattern, diffLabs, jsMatcher, literalOf, patternFromInput, rowLabel, suggestTexts, triedOrder } from './rules';
import type { Lab, LabCategory, LabRow } from './engine';

const row = (o: Partial<LabRow>): LabRow => ({
  date: '2026-01-01', amount: '-1', account: 'a', merchant: 'M', raw: 'M', file: 'f.csv', kind: 'spend',
  category: null, subcategory: null, hit: null, transfer: null, ...o,
});

describe('patterns', () => {
  it('reads plain text back from escaped patterns', () => {
    expect(literalOf('UBER EATS')).toBe('UBER EATS');
    expect(literalOf('(?i)e\\-Transfer TO')).toBe('e-Transfer TO');
    expect(literalOf('PETRO-?EARTH')).toBeNull();
    expect(describePattern('CAFE')).toBe('“CAFE”');
    expect(describePattern('VET |PET')).toBe('/VET |PET/');
  });
  it('turns input into category / rule patterns', () => {
    expect(patternFromInput(' A&W (Main) ', false)).toBe('A\\&W \\(Main\\)');
    expect(patternFromInput('Leela', true)).toBe('(?i)Leela');
    expect(patternFromInput('/HOVER FUND|SPACE BANK/', true)).toBe('(?i)HOVER FUND|SPACE BANK');
    expect(patternFromInput('/x+/', false)).toBe('x+');
    expect(jsMatcher('(?i)uber eats')({ merchant: 'UBER EATS TORONTO', raw: '' })).toBe(true);
    expect(jsMatcher('(bad')({ merchant: 'x', raw: '' })).toBe(false);
  });
  it('suggests useful texts', () => {
    const s = suggestTexts('TIM HORTONS #1234 TORONTO ON');
    expect(s).toContain('TIM HORTONS');
    expect(s).toContain('TIM HORTONS #1234 TORONTO ON');
    expect(suggestTexts('NETFLIX.COM')).toEqual(['NETFLIX.COM']);
    expect(suggestTexts('E-TRANSFER TO: JANE DOE')[0]).toBe('JANE DOE');
  });
  it('skips an account name the bank puts before every description', () => {
    const acct = [
      ...Array(19).fill('SPACE SAVER ACCOUNT - 1 INTEREST CREDIT INTEREST AMOUNT 0.02'),
      ...Array(5).fill('SPACE SAVER ACCOUNT - 1 OFFICIAL CHEQUE SERVICE CHARGE 10.00 OTHER REFERENCE #'),
      'SPACE SAVER ACCOUNT - 1 DEPOSIT',
    ];
    const cheque = 'SPACE SAVER ACCOUNT - 1 OFFICIAL CHEQUE SERVICE CHARGE 10.00 OTHER REFERENCE #';
    const n = boilerplateWords(cheque, acct);
    expect(n).toBe(5);
    expect(suggestTexts(cheque, n)[0]).toBe('OFFICIAL CHEQUE');
    expect(suggestTexts(cheque, n)).toContain('OFFICIAL CHEQUE SERVICE CHARGE 10.00 OTHER REFERENCE');
    // The interest rows keep their own words (they don't go on differently).
    expect(boilerplateWords(acct[0], acct)).toBe(5);
    // An account that is mostly one merchant keeps that merchant's name.
    const card = [...Array(9).fill('AMZN MKTP CA'), 'AMZN DIGITAL', 'SHELL 123'];
    expect(boilerplateWords('AMZN MKTP CA', card)).toBe(0);
    expect(boilerplateWords('AMZN MKTP CA', card.slice(0, 10))).toBe(0); // one word is a merchant, not an account name
    expect(boilerplateWords('X Y', ['X Y', 'X Z'])).toBe(0); // too few rows to tell
  });
});

describe('labs', () => {
  it('labels and diffs rows', () => {
    const a = row({}), b = row({ hit: { category: 'Food', subcategory: 'Coffee', pattern: 'M' }, category: 'Food', subcategory: 'Coffee' });
    expect(rowLabel(a)).toBe('not sorted');
    expect(rowLabel(row({ kind: 'transfer', transfer: { how: 'rule', other: 'RRSP', rule: 0 } }))).toBe('transfer ↔ RRSP');
    const lab = (rows: LabRow[]): Lab => ({ rows, categories: [], rules: [], targets: [] });
    const d = diffLabs(lab([a, a]), lab([b, a]));
    expect([...d.keys()]).toEqual(['not sorted → Food › Coffee']);
  });
  it('orders rules like the engine', () => {
    expect(triedOrder([{ pattern: 'a', to: 'x', priority: 1 }, { pattern: 'b', to: 'x', priority: 9 }, { pattern: 'c', to: 'x', priority: 9 }])).toEqual([1, 2, 0]);
  });
});

describe('ranges and near misses', () => {
  it('turns ticks into an amount range', () => {
    const all = [5, 12, 18, 40, 60];
    expect(rangeFromTicks([5, 12, 18], all)).toEqual({ lo: null, hi: 18, leaks: 0 });
    expect(rangeFromTicks([40, 60], all)).toEqual({ lo: 40, hi: null, leaks: 0 });
    expect(rangeFromTicks(all, all)).toEqual({ lo: null, hi: null, leaks: 0 });
    expect(rangeFromTicks([5, 60], all).leaks).toBe(3); // can't skip the middle
  });
  it('finds an existing text that almost sorts a row', () => {
    const cats: LabCategory[] = [
      { name: 'Takeout', kind: 'spend', subs: [
        { name: 'Restaurants', patterns: ['FISHY JOES'], ranged: [], amount_min: null, amount_max: null, priority: 0, source: 'yours' },
        { name: 'Coffee', patterns: ['STARBUCKS'], ranged: [], amount_min: null, amount_max: '20', priority: 0, source: 'builtin' },
      ] },
    ];
    expect(lcs('FISHY JOES', 'FISHY JOE GRILL #12')).toBe('FISHY JOE');
    const ms = nearMisses(row({ merchant: 'FISHY JOE GRILL', raw: 'FISHY JOE GRILL #4' }), cats);
    expect(ms).toMatchObject([{ why: 'close', pattern: 'FISHY JOES', common: 'FISHY JOE' }]);
    const sb = nearMisses(row({ merchant: 'STARBUCKS COFFEE', raw: 'STARBUCKS COFFEE #1', amount: '-46.00' }), cats);
    expect(sb).toMatchObject([{ why: 'range', subcategory: 'Coffee' }]);
    expect(sb[0].note).toMatch(/up to 20/);
    expect(nearMisses(row({ merchant: 'STARBUCKS COFFEE', amount: '-6' }), cats)).toEqual([]); // not a miss: it matches and fits
  });
});
