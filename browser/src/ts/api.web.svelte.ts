// browser/src/ts/api.web.svelte.ts
import { State, API_URL } from "./store.svelte";
import { fetchCached, initDbVersion } from './cache.js';
import { filesResultFrom } from "./transform";
import type { FilesResult, ScannedFile, TrashAction, TrashEntry } from "./models";

type AgeMini = { count: number; size: number; disk: number; mtime: number };
export type RawFolder = { path: string; users: Record<string, Record<'0'|'1'|'2', AgeMini>> };

// Types also consumed by desktop-only components (ScanPanel). Exported here
// so both transports share the same shape.
export type DriveInfo = {
  path: string;
  filesystem?: string;
  total_bytes?: number;
  used_bytes?: number;
};
export type ScanProgress = {
  stage: "scan" | "sum" | "index";
  percent: number;
  message?: string;
};

const DESKTOP_ONLY = "Desktop-only action invoked in web build.";

class Api {
  private baseUrl = `${API_URL}/`;
  public error: string = "";
  /** Mirrors /api/health.smtp_configured. Used to gate the Notify button in
   *  the cleanup panel. Populated lazily by probeHealth(). */
  public smtpConfigured: boolean = false;
  /** Mirrors /api/health.filesystems (the server's FILESYSTEMS). Reactive:
   *  the trash icon and "up" from a filesystem depend on it. */
  public filesystems: string[] = $state([]);
  /** Mirrors /api/health.trash_undo_hours: how long Keep can undo a trash. */
  public trashUndoHours: number = $state(24);
  /** Mirrors /api/health.trash_keep_days: how long moved items stay in .trash. */
  public trashKeepDays: number = $state(30);
  /** Mirrors /api/health.many_users_px: bar segment width (px) that merges
   *  into the "Many Users" block. 0 disables grouping. */
  public manyUsersPx: number = $state(2);
  /** Mirrors /api/health.many_users_color: color of that block. */
  public manyUsersColor: string = $state("#D2CCBC");
  /** Mirrors /api/health.bar_empty_color: background of the empty part of
   *  the stat bars. */
  public barEmptyColor: string = $state("#262626");

  private async request<T>(
    endpoint: string = "",
    method: "GET" | "POST" | "PUT" | "DELETE" = "GET",
    body?: unknown,
    use_cache?: boolean
  ): Promise<T | null> {
    try {
      this.error = "";

      const options: RequestInit = {
        method,
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${State.token}`,
        },
      };

      if (body)
        options.body = JSON.stringify(body);

      if (use_cache) {
        // Stale-while-revalidate with in-flight dedup; see cache.ts.
        return fetchCached<T>({
          url: `${this.baseUrl}${endpoint}`,
          init: options,
          key: endpoint,
          onError: (msg) => { this.error = msg; },
          onAuthFailure: () => State.logout(),
        });
      }

      const url = `${this.baseUrl}${endpoint}`
      const response = await fetch(url, options);
      const data: T = await response.json();
      if (!response.ok) {
        // 401/403 → session is gone. Fully clear it (clears localStorage and
        // the IndexedDB cache) so the next user doesn't see stale data and a
        // reload doesn't silently log us back in with the dead token.
        if (response.status === 401 || response.status === 403) {
          State.logout();
        }
        this.error = (data as any).detail || "Unknown API error";
        return null;
      }
      return data;
    } catch (err) {
      this.error = "API: Error in fetching data.";
      return null;
    }
  }

  async getUsers(): Promise<string[]> {
    let result = await this.request<string[]>('users', "GET", undefined, true)
    return result ?? []
  }

  async getFolders(path: string, users: string[], age?: number): Promise<RawFolder[]> {
    const pathParam = `path=${encodeURIComponent(path)}`
    const userParam = users.length > 0 ? `&users=${encodeURIComponent(users.join(','))}` : '';
    const ageParam = age !== undefined && age !== -1 ? `&age=${age}` : ''
    const url = `folders?${pathParam}${userParam}${ageParam}`
    let result = await this.request<RawFolder[]>(url, "GET", undefined, true)
    return result ?? []
  }

  async getFiles(path: string, users: string[], age?: number): Promise<FilesResult> {
    const pathParam = `path=${encodeURIComponent(path)}`
    const userParam = users.length > 0 ? `&users=${encodeURIComponent(users.join(','))}` : ''
    const ageParam = age !== undefined && age !== -1 ? `&age=${age}` : ''
    const url = `files?${pathParam}${userParam}${ageParam}`
    const result = await this.request<FilesResult | ScannedFile[]>(url, "GET", undefined, true)
    // A bare array is a pre-totals response (old server or cache entry).
    if (Array.isArray(result)) return filesResultFrom(result)
    return result ?? { files: [], totals: {} }
  }

  /** One-shot probe of /api/health. Caches the smtp_configured flag on the
   *  Api instance so the cleanup panel can gate the Notify button without
   *  calling /health on every render. Also seeds the cache's DB version
   *  (initDbVersion wipes the store when the DB was rebuilt), so it must run
   *  before the first cached request of a session. */
  async probeHealth(): Promise<void> {
    try {
      const resp = await fetch(`${this.baseUrl}health`, {
        headers: { Authorization: `Bearer ${State.token}` },
      });
      if (!resp.ok) return;
      const data = await resp.json();
      this.smtpConfigured = !!data?.smtp_configured;
      this.filesystems = Array.isArray(data?.filesystems) ? data.filesystems : [];
      if (typeof data?.trash_undo_hours === 'number') this.trashUndoHours = data.trash_undo_hours;
      if (typeof data?.trash_keep_days === 'number') this.trashKeepDays = data.trash_keep_days;
      if (typeof data?.many_users_px === 'number') this.manyUsersPx = data.many_users_px;
      if (typeof data?.many_users_color === 'string') this.manyUsersColor = data.many_users_color;
      if (typeof data?.bar_empty_color === 'string') this.barEmptyColor = data.bar_empty_color;
      await initDbVersion(typeof data?.db_version === 'string' ? data.db_version : null);
    } catch {
      /* network hiccup — leave smtpConfigured and the version at previous values */
    }
  }

  /** POST /api/cleanup/script. Returns the Python script as a Blob the
   *  caller can save via URL.createObjectURL. */
  async cleanupScript(
    username: string,
    paths: { path: string; size: number }[],
  ): Promise<Blob> {
    const resp = await fetch(`${this.baseUrl}cleanup/script`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${State.token}`,
      },
      body: JSON.stringify({ username, paths }),
    });
    if (!resp.ok) {
      const msg = await resp.text().catch(() => "");
      throw new Error(msg || `cleanup/script failed (${resp.status})`);
    }
    return resp.blob();
  }

  /** POST /api/cleanup/notify. Admin only on the server. */
  async cleanupNotify(
    username: string,
    paths: { path: string; size: number }[],
    message?: string,
  ): Promise<{ sent: boolean; to: string }> {
    const resp = await fetch(`${this.baseUrl}cleanup/notify`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${State.token}`,
      },
      body: JSON.stringify({ username, paths, message }),
    });
    if (!resp.ok) {
      const msg = await resp.text().catch(() => "");
      throw new Error(msg || `cleanup/notify failed (${resp.status})`);
    }
    return resp.json();
  }

  /** Server-side trash cart. Plain fetches, never the IndexedDB cache (a
   *  cached cart would be stale), and a 403 here means "not yours", not a
   *  dead session, so it must not log the user out like request() does. */
  private async trashFetch<T>(endpoint: string, method: "GET" | "POST" | "DELETE", body?: unknown): Promise<T> {
    const resp = await fetch(`${this.baseUrl}${endpoint}`, {
      method,
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${State.token}`,
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (resp.status === 401) State.logout();
    if (!resp.ok) {
      const msg = await resp.text().catch(() => "");
      throw new Error(msg || `${method} ${endpoint} failed (${resp.status})`);
    }
    return resp.json();
  }

  /** GET /api/trash: the queued items this user may manage. */
  async trashList(): Promise<TrashEntry[]> {
    return this.trashFetch<TrashEntry[]>("trash", "GET");
  }

  /** POST /api/trash: queue an item for the trash or the archive; returns
   *  the stored entry. Queuing it for the other action switches it. */
  async trashAdd(path: string, size: number, owner: string, action: TrashAction = "trash"): Promise<TrashEntry> {
    return this.trashFetch<TrashEntry>("trash", "POST", { path, size, owner, action });
  }

  /** DELETE /api/trash?path= — Keep (the 1-day undo). */
  async trashKeep(path: string): Promise<TrashEntry> {
    return this.trashFetch<TrashEntry>(`trash?path=${encodeURIComponent(path)}`, "DELETE");
  }

  // Desktop-only methods. Stubs in the web build — only callable from
  // components gated by __DESKTOP__. If the web bundle ever reaches them,
  // fail loudly so the bug is visible.
  async listDrives(): Promise<DriveInfo[]> { throw new Error(DESKTOP_ONLY); }
  async scan(_paths: string[]): Promise<string> { throw new Error(DESKTOP_ONLY); }
  async cancelScan(): Promise<void> { throw new Error(DESKTOP_ONLY); }
  async onScanProgress(_cb: (p: ScanProgress) => void): Promise<() => void> { throw new Error(DESKTOP_ONLY); }
  async revealPath(_p: string): Promise<void> { throw new Error(DESKTOP_ONLY); }
  async openTerminal(_p: string): Promise<void> { throw new Error(DESKTOP_ONLY); }
  async deletePath(_p: string): Promise<void> { throw new Error(DESKTOP_ONLY); }
  async getRecentPaths(): Promise<string[]> { throw new Error(DESKTOP_ONLY); }
  async setRecentPaths(_paths: string[]): Promise<void> { throw new Error(DESKTOP_ONLY); }
}

export const api = new Api();
