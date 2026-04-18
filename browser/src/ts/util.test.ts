// browser/src/ts/util.test.ts
import { describe, it, expect } from 'vitest';
import { humanBytes, humanCount, escapeHtml, getParent, capitalize } from './util';

describe('humanBytes', () => {
  it('returns "0 B" for zero / falsy input', () => {
    expect(humanBytes(0)).toBe('0 B');
    expect(humanBytes(NaN as any)).toBe('0 B');
  });

  it('formats bytes in decimal (k=1000) units', () => {
    expect(humanBytes(1_000)).toBe('1 KB');
    expect(humanBytes(1_500)).toBe('1.5 KB');
    expect(humanBytes(1_500_000)).toBe('1.5 MB');
    expect(humanBytes(2_500_000_000)).toBe('2.5 GB');
  });

  it('respects the decimals argument', () => {
    expect(humanBytes(1_234_567, 0)).toBe('1 MB');
    expect(humanBytes(1_234_567, 3)).toBe('1.235 MB');
  });
});

describe('humanCount', () => {
  it('compacts large numbers', () => {
    expect(humanCount(1234)).toBe('1.2K');
    expect(humanCount(1_500_000)).toBe('1.5M');
  });

  it('leaves small numbers unchanged', () => {
    expect(humanCount(42)).toBe('42');
  });
});

describe('escapeHtml', () => {
  it('escapes all five HTML-unsafe characters', () => {
    expect(escapeHtml(`<a href="x&y">'hi'</a>`))
      .toBe('&lt;a href=&quot;x&amp;y&quot;&gt;&#039;hi&#039;&lt;/a&gt;');
  });

  it('leaves safe strings alone', () => {
    expect(escapeHtml('plain text')).toBe('plain text');
  });
});

describe('getParent — Windows drive paths', () => {
  it('returns the parent of a nested path', () => {
    expect(getParent('C:\\Users\\San')).toBe('C:\\Users');
  });
  it('drive root is its own sort-of-parent (drive + \\)', () => {
    expect(getParent('C:\\Users')).toBe('C:\\');
  });
  it('drive root returns synthetic root', () => {
    expect(getParent('C:\\')).toBe('');
    expect(getParent('C:')).toBe('');
  });
});

describe('getParent — UNC paths', () => {
  it('strips last segment of a UNC path', () => {
    expect(getParent('\\\\server\\share\\dir')).toBe('\\\\server\\share');
    expect(getParent('\\\\server\\share')).toBe('\\\\server');
  });
  it('bare \\\\server falls back to synthetic root', () => {
    expect(getParent('\\\\server')).toBe('');
  });
});

describe('getParent — Unix paths', () => {
  it('returns parent of nested path', () => {
    expect(getParent('/var/log/x')).toBe('/var/log');
  });
  it('one level below root returns /', () => {
    expect(getParent('/var')).toBe('/');
  });
  it('/ returns synthetic root', () => {
    expect(getParent('/')).toBe('');
  });
});

describe('getParent — edge cases', () => {
  it('empty/invalid input returns ""', () => {
    expect(getParent('')).toBe('');
    expect(getParent('   ')).toBe('');
    expect(getParent(null as any)).toBe('');
    expect(getParent(undefined as any)).toBe('');
  });
});

describe('capitalize', () => {
  it('uppercases the first character', () => {
    expect(capitalize('hello')).toBe('Hello');
  });
  it('leaves already-capitalized strings unchanged', () => {
    expect(capitalize('Hello')).toBe('Hello');
  });
  it('returns "" for falsy input', () => {
    expect(capitalize('')).toBe('');
    expect(capitalize(null as any)).toBe('');
  });
});
