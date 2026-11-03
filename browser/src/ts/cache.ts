// browser/src/ts/cache.ts
import { openDB } from 'idb';

const DB_NAME = 'ApCacheDB';
const STORE_NAME = 'ap';
// v3: entry shape gained `version` (the DB build version the entry was
// cached under). Old {data, timestamp} rows can't be told apart from fresh
// ones, so the upgrade wipes the store and everything re-caches.
const DB_VERSION = 3;
// Soft expiry for same-version entries. The DB is immutable once built, so
// version matching alone would justify an unlimited TTL; the soft floor
// keeps a long-lived page session from serving data older than a server
// restart without ever hitting the network.
const SOFT_TTL_MS = 1 * 60 * 1000; // 1 minute
// Reserved entry holding the DB version the cache was last wiped for.
const VERSION_KEY = '__db_version';

export type CacheEntry = { data: unknown; timestamp: number; version: string | null };

// Server DB version, set once per page load by api.probeHealth(). While
// unknown (health not probed yet, no DB loaded, or fetch failure) staleness
// falls back to the soft TTL only — the pre-version behavior.
let serverVersion: string | null = null;
let versionKnown = false;

// Initialize or open the database
async function getDB() {
  return openDB(DB_NAME, DB_VERSION, {
    upgrade(db) {
      if (db.objectStoreNames.contains(STORE_NAME)) {
        db.deleteObjectStore(STORE_NAME);
      }
      db.createObjectStore(STORE_NAME);
    }
  });
}

/** One-shot per page load: record the server's DB version and wipe the
 *  store when it changed since the cache was last primed. A rebuilt DB has
 *  a new `built_at`, so this is what invalidates every cached entry across
 *  page loads. `null` (no DB loaded / probe failed) leaves the version
 *  unknown and callers on TTL-only semantics. Idempotent. */
export async function initDbVersion(v: string | null): Promise<void> {
  const db = await getDB();
  const prev = await db.get(STORE_NAME, VERSION_KEY) as CacheEntry | undefined;
  const prevVersion = prev?.version ?? null;
  if (prevVersion !== v) {
    await db.clear(STORE_NAME);
    await db.put(STORE_NAME, { data: null, timestamp: Date.now(), version: v }, VERSION_KEY);
  }
  serverVersion = v;
  versionKnown = true;
}

/** Pure staleness check, exported for unit tests.
 *  Stale = version mismatch (DB rebuilt, or entry written before the
 *  version was known) or past the soft TTL within the same version. */
export function isStale(entry: CacheEntry, now: number = Date.now()): boolean {
  if (versionKnown && entry.version !== serverVersion) return true;
  return now - entry.timestamp > SOFT_TTL_MS;
}

// Store data stamped with the current server version
export async function setCache(key: string, data: unknown) {
  const db = await getDB();
  await db.put(STORE_NAME, { data, timestamp: Date.now(), version: serverVersion }, key);
}

// Get a cached entry and whether it is stale (the caller decides to serve
// it anyway — stale-while-revalidate — or drop it)
export async function getCache(key: string): Promise<CacheEntry | null> {
  const db = await getDB();
  const entry = await db.get(STORE_NAME, key) as CacheEntry | undefined;
  return entry ?? null;
}

// Clear one or all cached entries
export async function clearCache(key: string) {
  const db = await getDB();
  await db.delete(STORE_NAME, key);
}

export async function clearAll() {
  const db = await getDB();
  await db.clear(STORE_NAME);
}

// --- stale-while-revalidate fetch with in-flight dedup ---

// One shared promise per cache key while a fetch is running. A second
// concurrent request for the same key awaits the first's promise instead of
// firing its own fetch. Only ever holds GETs: callers without use_cache
// (POST/PUT/DELETE) never reach fetchCached.
const inflight = new Map<string, Promise<unknown>>();

export interface FetchCachedOpts {
  url: string;
  init: RequestInit;
  /** Cache key = endpoint string, same as the entries in the store. */
  key: string;
  /** Transport hook for its `error` field. */
  onError?: (msg: string) => void;
  /** Transport hook for 401/403 (web transport: logout). */
  onAuthFailure?: () => void;
}

/** Stale-while-revalidate fetch. Fresh cache hit → served, no network.
 *  Stale hit → served immediately, refreshed in the background. Miss →
 *  awaits the network. Concurrent identical requests share one fetch. */
export async function fetchCached<T>(opts: FetchCachedOpts): Promise<T | null> {
  const entry = await getCache(opts.key);
  if (entry && opts.key !== VERSION_KEY) {
    if (!isStale(entry)) {
      return entry.data as T;
    }
    // Stale: serve it now, refresh in the background. A rejection is
    // already routed to onError/onAuthFailure inside revalidate.
    void revalidate<T>(opts).catch(() => {});
    return entry.data as T;
  }
  return await revalidate<T>(opts);
}

async function revalidate<T>(opts: FetchCachedOpts): Promise<T | null> {
  const existing = inflight.get(opts.key);
  if (existing) return existing as Promise<T | null>;

  const p = (async () => {
    try {
      const response = await fetch(opts.url, opts.init);
      const data = await response.json();
      if (!response.ok) {
        if (response.status === 401 || response.status === 403) {
          opts.onAuthFailure?.();
        }
        opts.onError?.((data as any)?.detail || "Unknown API error");
        return null;
      }
      await setCache(opts.key, data);
      return data as T;
    } catch {
      opts.onError?.("API: Error in fetching data.");
      return null;
    } finally {
      inflight.delete(opts.key);
    }
  })();

  inflight.set(opts.key, p);
  return p;
}
