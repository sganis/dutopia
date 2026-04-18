// browser/src/ts/deleteQueue.svelte.ts
//
// Targeted-deletion queue. Items are staged here by the "target for deletion"
// action on folder/file rows, then actually moved to the trash via the
// DeletePanel ("Delete" per-item or "Delete all" in parallel).
//
// Queue state is persisted to localStorage so it survives app restarts — an
// interrupted delete session (or one that spans days as the user curates a
// cleanup list) doesn't lose what was staged.

import { api } from "$api";

export type QueueStatus = "queued" | "deleting" | "error";

export type QueueItem = {
  path: string;
  /** Disk bytes the item occupies. Shown per-row and summed for the header
   *  total so the user can see roughly how much space the queue will free. */
  size: number;
  status: QueueStatus;
  error?: string;
};

const STORAGE_KEY = "deleteQueue.v1";

function loadFromStorage(): QueueItem[] {
  if (typeof localStorage === "undefined") return [];
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    // Items that were mid-delete when the app closed didn't actually get
    // trashed (we'd have removed them from the queue on success) — reset
    // them to "queued" so the user can re-trigger the delete.
    return parsed
      .filter((i) => typeof i?.path === "string")
      .map((i) => ({
        path: i.path,
        size: Number(i.size) || 0,
        status: (i.status === "error" ? "error" : "queued") as QueueStatus,
        error: i.error,
      }));
  } catch {
    return [];
  }
}

function saveToStorage() {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(deleteQueue.items));
  } catch {
    /* quota / private mode — non-fatal */
  }
}

export const deleteQueue = $state({
  items: loadFromStorage(),
  panelOpen: false,
});

function indexOf(path: string): number {
  return deleteQueue.items.findIndex((i) => i.path === path);
}

export function addToQueue(path: string, size: number = 0) {
  if (indexOf(path) >= 0) return;
  deleteQueue.items.push({ path, size, status: "queued" });
  saveToStorage();
}

/** "Keep" button — remove from queue without deleting. */
export function removeFromQueue(path: string) {
  deleteQueue.items = deleteQueue.items.filter((i) => i.path !== path);
  saveToStorage();
}

/** Promise that resolves after the trash call completes (success or error).
 *  Used by both per-item delete and delete-all (Promise.all parallel run). */
export async function deleteOne(path: string): Promise<void> {
  const i = indexOf(path);
  if (i < 0) return;
  deleteQueue.items[i].status = "deleting";
  deleteQueue.items[i].error = undefined;
  saveToStorage();
  try {
    await api.deletePath(path);
    deleteQueue.items = deleteQueue.items.filter((x) => x.path !== path);
  } catch (err: any) {
    const j = indexOf(path);
    if (j >= 0) {
      deleteQueue.items[j].status = "error";
      deleteQueue.items[j].error = err?.message ?? String(err);
    }
  } finally {
    saveToStorage();
  }
}

export async function deleteAll(): Promise<void> {
  const targets = deleteQueue.items
    .filter((i) => i.status !== "deleting")
    .map((i) => i.path);
  await Promise.all(targets.map((p) => deleteOne(p)));
}
