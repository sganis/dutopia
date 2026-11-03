// browser/src/ts/cleanupQueue.svelte.ts
//
// Browser-build trash cart, kept on the server (GET/POST/DELETE /api/trash)
// so the nightly cron can see it: an item queued here is moved to
// `<fs>/.trash` once it has waited 24h. Keep (removeFromCleanupQueue) is the
// undo for that day. The script/notify actions stay as extra ways to ask
// the owner to clean up; they no longer empty the cart.
//
// Each item carries its owner so the panel can filter by user (same UX
// pattern as the main app's user dropdown) and the script/notify endpoints
// know which user to target.

import { api } from "$api";
import type { TrashEntry } from "./models";

export type CleanupStatus = "queued" | "requesting" | "requested" | "error";

export type CleanupItem = TrashEntry & {
  status: CleanupStatus;
  error?: string;
};

export const cleanupQueue = $state({
  items: [] as CleanupItem[],
  panelOpen: false,
});

function toItem(e: TrashEntry): CleanupItem {
  return { ...e, status: "queued" };
}

/** Replace the cart with the server's view of it. */
export async function loadCleanupQueue(): Promise<void> {
  const entries = await api.trashList();
  cleanupQueue.items = entries.map(toItem);
}

/** Queue an item on the server. Throws (cart unchanged) when refused. */
export async function addToCleanupQueue(path: string, size: number, owner: string): Promise<void> {
  if (!owner) return;
  const e = await api.trashAdd(path, size, owner);
  if (!cleanupQueue.items.some((i) => i.path === e.path)) {
    cleanupQueue.items.push(toItem(e));
  }
}

/** Per-row "Keep": take the item out of the server queue (the undo). */
export async function removeFromCleanupQueue(path: string): Promise<void> {
  await api.trashKeep(path);
  cleanupQueue.items = cleanupQueue.items.filter((i) => i.path !== path);
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
 *  the group as "requested" on success; the items stay queued. */
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
    markGroup(owner, "requested");
  } catch (err: any) {
    markGroup(owner, "error", err?.message ?? String(err));
    throw err;
  }
}

/** Request the backend to email the owner. Admin only (enforced by the
 *  server). Marks the group as "requested"; the items stay queued. */
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
    markGroup(owner, "requested");
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
