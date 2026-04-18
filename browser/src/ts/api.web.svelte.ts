// browser/src/ts/api.web.svelte.ts
import { State, API_URL } from "./store.svelte";
import { getCache, setCache } from './cache.js';

type AgeMini = { count: number; size: number; disk: number; mtime: number };
type ScannedFile = { path: string; size: number; modified: string; owner: string };
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
        let data = await getCache(endpoint);
        if (data) {
          return data
        }
      }
      const url = `${this.baseUrl}${endpoint}`
      const response = await fetch(url, options);
      const data: T = await response.json();
      if (!response.ok) {
        // do this to redirect to login
        State.token = null
        this.error = (data as any).detail || "Unknown API error";
        return null;
      }
      await setCache(endpoint, data)
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

  async getFiles(path: string, users: string[], age?: number): Promise<ScannedFile[]> {
    const pathParam = `path=${encodeURIComponent(path)}`
    const userParam = users.length > 0 ? `&users=${encodeURIComponent(users.join(','))}` : ''
    const ageParam = age !== undefined && age !== -1 ? `&age=${age}` : ''
    const url = `files?${pathParam}${userParam}${ageParam}`
    let result = await this.request<ScannedFile[]>(url, "GET", undefined, true)
    return result ?? []
  }

  /** One-shot probe of /api/health. Caches the smtp_configured flag on the
   *  Api instance so the cleanup panel can gate the Notify button without
   *  calling /health on every render. */
  async probeHealth(): Promise<void> {
    try {
      const resp = await fetch(`${this.baseUrl}health`, {
        headers: { Authorization: `Bearer ${State.token}` },
      });
      if (!resp.ok) return;
      const data = await resp.json();
      this.smtpConfigured = !!data?.smtp_configured;
    } catch {
      /* network hiccup — leave smtpConfigured at previous value */
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
