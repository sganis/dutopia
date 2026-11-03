// browser/src/ts/transform.ts

import type {
  AgeFilterType,
  FileTotals,
  FilesResult,
  FolderItem,
  RawFolder,
  ScannedFile,
  UserStatsJson,
} from "./models";

const toNum = (v: any) => {
  const n = Number(v);
  return Number.isFinite(n) ? n : 0;
};

export function transformFolders(raw: RawFolder[], filter: AgeFilterType): FolderItem[] {
  const ages: string[] = filter === -1 ? ["0", "1", "2"] : [String(filter)];

  return (raw ?? [])
    .map((rf) => {
      const usersAgg: Record<string, UserStatsJson> = {};
      let total_count = 0;
      let total_size = 0;
      let total_disk = 0;
      let total_linked = 0;
      let max_atime = 0;
      let max_mtime = 0;

      const userEntries = Object.entries(rf.users ?? {});
      for (const [uname, agesMap] of userEntries) {
        let u_count = 0,
          u_disk = 0,
          u_size = 0,
          u_linked = 0,
          u_atime = 0,
          u_mtime = 0;

        for (const ageKey of ages) {
          const a = (agesMap as any)[ageKey];
          if (!a) continue;
          u_count += toNum(a.count);
          u_disk += toNum(a.disk);
          u_size += toNum(a.size);
          u_linked += toNum(a.linked);
          u_atime = Math.max(u_atime, toNum(a.atime));
          u_mtime = Math.max(u_mtime, toNum(a.mtime));
        }

        usersAgg[uname] = {
          username: uname,
          count: u_count,
          disk: u_disk,
          size: u_size,
          linked: u_linked,
          atime: u_atime,
          mtime: u_mtime,
        };

        total_count += u_count;
        total_disk += u_disk;
        total_size += u_size;
        total_linked += u_linked;
        max_atime = Math.max(max_atime, u_atime);
        max_mtime = Math.max(max_mtime, u_mtime);
      }

      return {
        path: rf.path,
        total_count,
        total_disk,
        total_size,
        total_linked,
        accessed: max_atime,
        modified: max_mtime,
        users: usersAgg,
      } as FolderItem;
    })
    .filter((rf) => rf.path);
}

/** Totals of a complete file list (desktop / older servers send no totals). */
export function filesResultFrom(files: ScannedFile[]): FilesResult {
  const totals: Record<string, FileTotals> = {};
  for (const f of files ?? []) {
    if (!f.owner) continue;
    const t = (totals[f.owner] ??= { count: 0, size: 0, accessed: 0, modified: 0 });
    t.count += 1;
    t.size += toNum(f.size);
    t.accessed = Math.max(t.accessed, toNum(f.accessed));
    t.modified = Math.max(t.modified, toNum(f.modified));
  }
  return { files: files ?? [], totals };
}

/** Totals of the current folder: its subfolders plus its own files. Files
 *  come from the per-owner totals, not the (possibly short) file list. */
export function aggregatePathTotals(
  foldersArr: FolderItem[],
  fileTotals: Record<string, FileTotals>,
  p: string
): FolderItem {
  let total_count = 0;
  let total_disk = 0;
  let total_size = 0;
  let total_linked = 0;
  let accessed = 0;
  let modified = 0;
  const aggUsers: Record<string, UserStatsJson> = {};
  const userEntry = (uname: string) =>
    (aggUsers[uname] ??= { username: uname, count: 0, disk: 0, size: 0, linked: 0, atime: 0, mtime: 0 });

  for (const f of foldersArr ?? []) {
    total_count += toNum(f?.total_count);
    total_disk += toNum(f?.total_disk);
    total_size += toNum(f?.total_size);
    total_linked += toNum(f?.total_linked);
    if (f.accessed > accessed) accessed = f.accessed;
    if (f.modified > modified) modified = f.modified;

    for (const [uname, data] of Object.entries(f?.users ?? {})) {
      const d = data as UserStatsJson;
      const u = userEntry(uname);
      u.count += toNum(d.count);
      u.disk += toNum(d.disk);
      u.size += toNum(d.size);
      u.linked += toNum(d.linked);
      u.atime = Math.max(u.atime, toNum(d.atime));
      u.mtime = Math.max(u.mtime, toNum(d.mtime));
    }
  }

  for (const [owner, t] of Object.entries(fileTotals ?? {})) {
    const count = toNum(t.count);
    const size = toNum(t.size);
    total_count += count;
    total_disk += size;
    total_size += size;
    if (t.accessed > accessed) accessed = t.accessed;
    if (t.modified > modified) modified = t.modified;
    if (!owner) continue;
    const u = userEntry(owner);
    u.count += count;
    u.disk += size;
    u.size += size;
    u.atime = Math.max(u.atime, toNum(t.accessed));
    u.mtime = Math.max(u.mtime, toNum(t.modified));
  }

  return { path: p, total_count, total_disk, total_size, total_linked, accessed, modified, users: aggUsers };
}

export function safeNumber(v: any) {
  return toNum(v);
}
