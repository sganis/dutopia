// browser/src/ts/api.tauri.svelte.ts
//
// Tauri transport for the dutopia desktop build. Backend queries run
// against a local SQLite pool — no HTTP, no round-trip latency — so we
// skip the IndexedDB cache entirely. This avoids a whole class of
// staleness bugs (cache returning empty pre-scan results after a scan
// completes).

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

type AgeMini = { count: number; size: number; disk: number; mtime: number };
type ScannedFile = { path: string; size: number; modified: string; owner: string };
export type RawFolder = { path: string; users: Record<string, Record<"0" | "1" | "2", AgeMini>> };

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

const WEB_ONLY = "Web-only action invoked in desktop build.";

class Api {
  public error: string = "";
  /** Present so the cleanup panel (web-only) compiles against this
   *  transport. Desktop never reads it. */
  public smtpConfigured: boolean = false;

  private async call<T>(cmd: string, args?: Record<string, unknown>): Promise<T | null> {
    try {
      this.error = "";
      return await invoke<T>(cmd, args);
    } catch (err: any) {
      this.error = typeof err === "string" ? err : err?.message ?? "Tauri command failed";
      console.error(`[api.tauri] ${cmd} failed`, err);
      return null;
    }
  }

  async getUsers(): Promise<string[]> {
    const result = await this.call<string[]>("get_users", {});
    console.log("[api.tauri] getUsers →", result);
    return result ?? [];
  }

  async getFolders(path: string, users: string[], age?: number): Promise<RawFolder[]> {
    const ageKey = age !== undefined && age !== -1 ? age : null;
    const result = await this.call<RawFolder[]>("get_folders", { path, users, age: ageKey });
    console.log(`[api.tauri] getFolders path=${JSON.stringify(path)} → ${result?.length ?? "null"} items`);
    return result ?? [];
  }

  async getFiles(path: string, users: string[], age?: number): Promise<ScannedFile[]> {
    const ageKey = age !== undefined && age !== -1 ? age : null;
    const result = await this.call<ScannedFile[]>("get_files", { path, users, age: ageKey });
    return result ?? [];
  }

  async listDrives(): Promise<DriveInfo[]> {
    const result = await invoke<DriveInfo[]>("list_drives");
    return result ?? [];
  }

  async scan(paths: string[]): Promise<string> {
    console.log("[api.tauri] scan start", paths);
    const result = await invoke<string>("scan", { paths });
    console.log("[api.tauri] scan done", result);
    return result;
  }

  async cancelScan(): Promise<void> {
    await invoke("cancel_scan");
  }

  async onScanProgress(cb: (p: ScanProgress) => void): Promise<() => void> {
    const unlisten = await listen<ScanProgress>("scan-progress", (evt: { payload: ScanProgress }) => cb(evt.payload));
    return unlisten;
  }

  async revealPath(path: string): Promise<void> {
    await invoke("reveal_in_path", { path });
  }

  async openTerminal(path: string): Promise<void> {
    await invoke("open_terminal", { path });
  }

  async deletePath(path: string): Promise<void> {
    await invoke("delete_path", { path });
  }

  async getRecentPaths(): Promise<string[]> {
    try {
      return await invoke<string[]>("get_recent_paths");
    } catch {
      return [];
    }
  }

  async setRecentPaths(paths: string[]): Promise<void> {
    try { await invoke("set_recent_paths", { paths }); } catch (err) { console.error("setRecentPaths", err); }
  }

  // Web-only cleanup-request endpoints. The desktop build deletes directly
  // via delete_path, so these are stubs; the CleanupPanel is only mounted
  // under `!__DESKTOP__` and should never reach them.
  async probeHealth(): Promise<void> { /* noop on desktop */ }
  async cleanupScript(_username: string, _paths: { path: string; size: number }[]): Promise<Blob> {
    throw new Error(WEB_ONLY);
  }
  async cleanupNotify(
    _username: string,
    _paths: { path: string; size: number }[],
    _message?: string,
  ): Promise<{ sent: boolean; to: string }> {
    throw new Error(WEB_ONLY);
  }
}

export const api = new Api();
