// browser/src/ts/transform.test.ts
import { describe, it, expect } from 'vitest';
import { transformFolders, aggregatePathTotals, filesResultFrom, safeNumber } from './transform';
import type { RawFolder, FolderItem, ScannedFile } from './models';

const mkAge = (count: number, size: number, atime = 100, mtime = 200) => ({
  count,
  disk: size,
  size,
  linked: 0,
  atime,
  mtime,
});

describe('safeNumber', () => {
  it('coerces strings to numbers', () => {
    expect(safeNumber('42')).toBe(42);
  });
  it('returns 0 for non-finite input', () => {
    expect(safeNumber('x')).toBe(0);
    expect(safeNumber(undefined)).toBe(0);
    expect(safeNumber(null)).toBe(0);
  });
});

describe('transformFolders', () => {
  const raw: RawFolder[] = [
    {
      path: '/a',
      users: {
        alice: { '0': mkAge(10, 1000), '2': mkAge(5, 500, 50, 300) },
        bob: { '1': mkAge(3, 300) },
      },
    },
    { path: '', users: {} }, // should be filtered out (falsy path)
  ];

  it('aggregates across all age buckets when filter = -1', () => {
    const out = transformFolders(raw, -1);
    expect(out).toHaveLength(1);
    const f = out[0];
    expect(f.path).toBe('/a');
    expect(f.total_count).toBe(18); // 10 + 5 + 3
    expect(f.total_size).toBe(1800);
    expect(f.users.alice.count).toBe(15);
    expect(f.users.alice.size).toBe(1500);
    expect(f.users.bob.count).toBe(3);
  });

  it('filters to a single age bucket', () => {
    const out = transformFolders(raw, 0);
    const f = out[0];
    expect(f.total_count).toBe(10); // only alice age=0
    expect(f.users.alice.count).toBe(10);
    expect(f.users.bob.count).toBe(0); // bob has no age=0 data
  });

  it('tracks max atime/mtime across buckets', () => {
    const out = transformFolders(raw, -1);
    const f = out[0];
    // alice age=0 (atime=100, mtime=200); alice age=2 (atime=50, mtime=300)
    expect(f.users.alice.atime).toBe(100);
    expect(f.users.alice.mtime).toBe(300);
    expect(f.accessed).toBe(100);
    expect(f.modified).toBe(300);
  });

  it('drops entries with falsy path', () => {
    expect(transformFolders(raw, -1).map((f) => f.path)).toEqual(['/a']);
  });

  it('handles empty / null input gracefully', () => {
    expect(transformFolders([], -1)).toEqual([]);
    expect(transformFolders(null as any, -1)).toEqual([]);
  });
});

describe('aggregatePathTotals', () => {
  const folder: FolderItem = {
    path: '/a',
    total_count: 5,
    total_disk: 500,
    total_size: 500,
    total_linked: 0,
    accessed: 100,
    modified: 200,
    users: {
      alice: { username: 'alice', count: 5, disk: 500, size: 500, linked: 0, atime: 100, mtime: 200 },
    },
  };

  // alice owns 3 files in /a: f.txt is listed, two small ones are not.
  const totals = { alice: { count: 3, size: 100, accessed: 150, modified: 250 } };

  it('sums folder + file totals', () => {
    const agg = aggregatePathTotals([folder], totals, '/a');
    expect(agg.path).toBe('/a');
    expect(agg.total_count).toBe(8);
    expect(agg.total_size).toBe(600);
    expect(agg.total_disk).toBe(600);
  });

  it('takes max of accessed/modified across folders and files', () => {
    const agg = aggregatePathTotals([folder], totals, '/a');
    expect(agg.accessed).toBe(150);
    expect(agg.modified).toBe(250);
  });

  it('merges users across folder stats and file owner', () => {
    const agg = aggregatePathTotals([folder], totals, '/a');
    expect(agg.users.alice.count).toBe(8);
    expect(agg.users.alice.size).toBe(600);
    expect(agg.users.alice.atime).toBe(150);
  });

  it('handles empty inputs', () => {
    const agg = aggregatePathTotals([], {}, '/x');
    expect(agg.total_count).toBe(0);
    expect(agg.users).toEqual({});
    expect(agg.path).toBe('/x');
  });

  it('counts orphan files with no owner but adds no user', () => {
    const agg = aggregatePathTotals([], { '': { count: 1, size: 5, accessed: 0, modified: 0 } }, '/a');
    expect(agg.total_count).toBe(1);
    expect(agg.users).toEqual({});
  });
});

describe('filesResultFrom', () => {
  const mk = (path: string, owner: string, size: number, modified: number): ScannedFile => ({
    path,
    owner,
    size,
    accessed: 0,
    modified,
  });

  it('totals a complete list per owner', () => {
    const files = [mk('/a/x', 'alice', 10, 5), mk('/a/y', 'alice', 30, 9), mk('/a/z', 'bob', 20, 1)];
    const r = filesResultFrom(files);
    expect(r.files).toBe(files);
    expect(r.totals.alice).toEqual({ count: 2, size: 40, accessed: 0, modified: 9 });
    expect(r.totals.bob.count).toBe(1);
  });

  it('skips files with no owner', () => {
    expect(filesResultFrom([mk('/a/x', '', 1, 1)]).totals).toEqual({});
  });
});
