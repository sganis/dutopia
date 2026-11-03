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

import { SvelteSet } from "svelte/reactivity";
import { api } from "$api";
import type { TrashEntry, TrashOutcome } from "./models";

export type CleanupStatus = "queued" | "requesting" | "requested" | "error";

export type CleanupItem = TrashEntry & {
  status: CleanupStatus;
  error?: string;
};

export const cleanupQueue = $state({
  items: [] as CleanupItem[],
  /** What the nightly run did with items that left the queue, newest first. */
  history: [] as TrashOutcome[],
  panelOpen: false,
});

/** Paths in the cart, for the per-row "in trash" check: a Set so a 2000-row
 *  listing does not scan a 20k-item admin cart once per row. */
const queuedPaths = new SvelteSet<string>();

function setItems(items: CleanupItem[]) {
  cleanupQueue.items = items;
  queuedPaths.clear();
  for (const i of items) queuedPaths.add(i.path);
}

function toItem(e: TrashEntry): CleanupItem {
  return { ...e, status: "queued" };
}

/** Replace the cart with the server's view of it. */
export async function loadCleanupQueue(): Promise<void> {
  const entries = await api.trashList();
  setItems(entries.map(toItem));
}

/** Replace the history with the server's (GET /api/trash/history). */
export async function loadTrashHistory(): Promise<void> {
  cleanupQueue.history = await api.trashHistory();
}

/** Queue an item on the server, then re-read the cart: a folder absorbs
 *  its queued children. Throws (cart unchanged) when refused. */
export async function addToCleanupQueue(path: string, size: number, owner: string): Promise<void> {
  if (!owner) return;
  await api.trashAdd(path, size, owner);
  await loadCleanupQueue();
}

export function isQueued(path: string): boolean {
  return queuedPaths.has(path);
}

/** The row's trash icon: queue the item, or Keep it when already queued. */
export async function toggleTrash(path: string, size: number, owner: string): Promise<"queued" | "kept"> {
  if (isQueued(path)) {
    await removeFromCleanupQueue(path);
    return "kept";
  }
  await addToCleanupQueue(path, size, owner);
  return "queued";
}

/** Per-row "Keep": take the item out of the server queue (the undo). */
export async function removeFromCleanupQueue(path: string): Promise<void> {
  await api.trashKeep(path);
  setItems(cleanupQueue.items.filter((i) => i.path !== path));
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
