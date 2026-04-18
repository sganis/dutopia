// browser/src/ts/cleanupQueue.svelte.ts
//
// Browser-build queue for "request cleanup" actions.
//
// Mirrors deleteQueue.svelte.ts in shape but ends with non-destructive
// actions — the server cannot delete on behalf of arbitrary users in a
// read-only cluster deployment. Each queued item carries its owner so the
// panel can filter by user (same UX pattern as the main app's user
// dropdown) and the script/notify endpoints know which user to target.

import { api } from "$api";

export type CleanupStatus = "queued" | "requesting" | "requested" | "error";

export type CleanupItem = {
  path: string;
  /** Disk bytes the item occupies. Summed for header totals. */
  size: number;
  /** Owning username — required. Folders with mixed ownership are blocked
   *  at the row-action level so we never queue one without a single owner. */
  owner: string;
  status: CleanupStatus;
  error?: string;
};

const STORAGE_KEY = "cleanupQueue.v1";

function loadFromStorage(): CleanupItem[] {
  if (typeof localStorage === "undefined") return [];
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((i) => typeof i?.path === "string" && typeof i?.owner === "string")
      .map((i) => ({
        path: i.path,
        size: Number(i.size) || 0,
        owner: i.owner,
        // An in-flight request that was interrupted by a page reload didn't
        // actually complete — drop back to queued so the user can retry.
        status: (i.status === "error" ? "error" : "queued") as CleanupStatus,
        error: i.error,
      }));
  } catch {
    return [];
  }
}

function saveToStorage() {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(cleanupQueue.items));
  } catch {
    /* quota / private mode — non-fatal */
  }
}

export const cleanupQueue = $state({
  items: loadFromStorage(),
  panelOpen: false,
});

function indexOf(path: string): number {
  return cleanupQueue.items.findIndex((i) => i.path === path);
}

export function addToCleanupQueue(path: string, size: number, owner: string) {
  if (!owner) return;
  if (indexOf(path) >= 0) return;
  cleanupQueue.items.push({ path, size, owner, status: "queued" });
  saveToStorage();
}

/** Per-row "Keep" — remove from queue without requesting cleanup. */
export function removeFromCleanupQueue(path: string) {
  cleanupQueue.items = cleanupQueue.items.filter((i) => i.path !== path);
  saveToStorage();
}

/** Clear every item owned by a given user (used after a successful
 *  script download or email-notify). */
export function clearUser(owner: string) {
  cleanupQueue.items = cleanupQueue.items.filter((i) => i.owner !== owner);
  saveToStorage();
}

/** Distinct owners currently in the queue, sorted alphabetically for the
 *  dropdown options. */
export function distinctOwners(): string[] {
  const set = new Set<string>();
  for (const i of cleanupQueue.items) set.add(i.owner);
  return Array.from(set).sort();
}

/** Items filtered by owner. Pass null for "All users". */
export function filteredItems(owner: string | null): CleanupItem[] {
  if (!owner) return cleanupQueue.items.slice();
  return cleanupQueue.items.filter((i) => i.owner === owner);
}

export function totalsFor(owner: string | null): { count: number; size: number } {
  const items = filteredItems(owner);
  let size = 0;
  for (const i of items) size += i.size || 0;
  return { count: items.length, size };
}

/** Download the server-generated Python script for a user's items. Marks
 *  the group as "requested" on success and removes them from the queue. */
export async function downloadScript(owner: string): Promise<void> {
  const items = filteredItems(owner);
  if (items.length === 0) return;
  try {
    markGroup(owner, "requesting");
    const blob = await api.cleanupScript(
      owner,
      items.map((i) => ({ path: i.path, size: i.size })),
    );
    saveBlob(blob, `cleanup-${safeFilename(owner)}-${yyyymmdd()}.py`);
    clearUser(owner);
  } catch (err: any) {
    markGroup(owner, "error", err?.message ?? String(err));
    throw err;
  }
}

/** Request the backend to email the owner. Admin only (enforced by the
 *  server). Clears the group on success. */
export async function notifyUser(owner: string, message?: string): Promise<string> {
  const items = filteredItems(owner);
  if (items.length === 0) return "";
  try {
    markGroup(owner, "requesting");
    const resp = await api.cleanupNotify(
      owner,
      items.map((i) => ({ path: i.path, size: i.size })),
      message,
    );
    clearUser(owner);
    return resp?.to ?? "";
  } catch (err: any) {
    markGroup(owner, "error", err?.message ?? String(err));
    throw err;
  }
}

function markGroup(owner: string, status: CleanupStatus, error?: string) {
  for (const i of cleanupQueue.items) {
    if (i.owner === owner) {
      i.status = status;
      i.error = error;
    }
  }
  saveToStorage();
}

function saveBlob(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function safeFilename(s: string): string {
  return s.replace(/[^A-Za-z0-9_-]/g, "_");
}

function yyyymmdd(): string {
  const d = new Date();
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}${m}${day}`;
}
