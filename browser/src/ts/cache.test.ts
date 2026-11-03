// browser/src/ts/cache.test.ts
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { isStale, initDbVersion, type CacheEntry } from './cache';

// isStale reads module state (serverVersion / versionKnown), so each test
// primes it through initDbVersion. That opens IndexedDB, which jsdom lacks —
// mock the `idb` wrapper so initDbVersion runs against a fake store.
vi.mock('idb', () => {
  const store = new Map<string, unknown>();
  const fakeDb = {
    get: async (_s: string, key: string) => store.get(key),
    put: async (_s: string, value: unknown, key: string) => { store.set(key, value); },
    clear: async () => store.clear(),
    delete: async (_s: string, key: string) => { store.delete(key); },
  };
  return { openDB: async () => fakeDb };
});

const NOW = 1_000_000_000_000;

function entry(version: string | null, ageMs: number): CacheEntry {
  return { data: {}, timestamp: NOW - ageMs, version };
}

describe('isStale', () => {
  beforeEach(async () => {
    await initDbVersion('v1');
  });

  it('version mismatch is stale', () => {
    expect(isStale(entry('v2', 0), NOW)).toBe(true);
  });

  it('version match within TTL is fresh', () => {
    expect(isStale(entry('v1', 30_000), NOW)).toBe(false);
  });

  it('version match past TTL is stale', () => {
    expect(isStale(entry('v1', 120_000), NOW)).toBe(true);
  });

  it('unknown version (entry predating the probe) is stale', () => {
    expect(isStale(entry(null, 0), NOW)).toBe(true);
  });
});

describe('isStale with unknown server version (TTL fallback)', () => {
  it('ignores the version and uses TTL only', async () => {
    // The probe returned a null version (server without a DB / failed
    // fetch): the null entry is in fact current, so the TTL applies
    // regardless of the version fields.
    await initDbVersion(null);
    expect(isStale(entry('anything', 30_000), NOW)).toBe(true); // mismatch
    expect(isStale(entry(null, 30_000), NOW)).toBe(false);    // match
    expect(isStale(entry('anything', 120_000), NOW)).toBe(true); // TTL
  });
});
