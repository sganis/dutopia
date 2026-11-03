// browser/src/ts/trash.ts
//
// Pure trash rules shared by the page and the cleanup panel. They mirror the
// server (src/bin/duapi/trash.rs) so the trash icon only shows where
// POST /api/trash would accept the item; the server still decides.

/** Hours an item waits in the queue before the nightly cron may take it. */
export const UNDO_HOURS = 24;

export type TrashViewer = {
  username: string;
  isAdmin: boolean;
  roots: string[];
  filesystems: string[];
};

/** `path` is `root` or lies under it (`/`-bounded, like authz::path_in_roots). */
export function inRoot(root: string, path: string): boolean {
  const r = root.replace(/\/+$/, "");
  const p = path.replace(/\/+$/, "");
  if (r === "") return p === "" || path.startsWith("/");
  return p === r || p.startsWith(r + "/");
}

/** The FILESYSTEMS entry holding `path` (longest prefix), or null. */
export function fsOf(filesystems: string[], path: string): string | null {
  let best: string | null = null;
  for (const fs of filesystems) {
    if (inRoot(fs, path) && (best === null || fs.length > best.length)) best = fs;
  }
  return best;
}

/** May `viewer` trash the item `path` owned by `owner`? */
export function canTrash(viewer: TrashViewer, path: string, owner: string | null): boolean {
  // UNK = a UID with no account (dusum/dudb): the cron cannot check it.
  if (!owner || owner === "UNK") return false;
  const p = path.replace(/\/+$/, "");
  const fs = fsOf(viewer.filesystems, p);
  if (fs === null || p === fs.replace(/\/+$/, "")) return false;
  const base = fs === "/" ? "" : fs;
  if (inRoot(`${base}/.trash`, p) || inRoot(`${base}/.stats`, p)) return false;
  return viewer.isAdmin
    || viewer.roots.some((r) => inRoot(r, p))
    || owner === viewer.username;
}

/** When the cron may move an item queued at `queuedAt` (unix seconds), as ms. */
export function trashAfter(queuedAt: number): number {
  return (queuedAt + UNDO_HOURS * 3600) * 1000;
}
