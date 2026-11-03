// browser/src/ts/transform.ts

import type { AgeFilterType, FolderItem, RawFolder, ScannedFile, UserStatsJson } from "./models";

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

export function aggregatePathTotals(
  foldersArr: FolderItem[],
  filesArr: ScannedFile[],
  p: string
): FolderItem {
  let total_count = 0;
  let total_disk = 0;
  let total_size = 0;
  let total_linked = 0;
  let accessed = 0;
  let modified = 0;
  const aggUsers: Record<string, UserStatsJson> = {};

  for (const f of foldersArr ?? []) {
    total_count += toNum(f?.total_count);
    total_disk += toNum(f?.total_disk);
    total_size += toNum(f?.total_size);
    total_linked += toNum(f?.total_linked);
    if (f.accessed > accessed) accessed = f.accessed;
    if (f.modified > modified) modified = f.modified;

    const u = f?.users ?? {};
    for (const [uname, data] of Object.entries(u)) {
      const d = data as UserStatsJson;
      const prev = aggUsers[uname] ?? {
        username: uname,
        count: 0,
        disk: 0,
        size: 0,
        linked: 0,
        atime: 0,
        mtime: 0,
      };
      aggUsers[uname] = {
        username: uname,
        count: prev.count + toNum(d.count),
        disk: prev.disk + toNum(d.disk),
        size: prev.size + toNum(d.size),
        linked: prev.linked + toNum(d.linked),
        atime: Math.max(prev.atime, toNum(d.atime)),
        mtime: Math.max(prev.mtime, toNum(d.mtime)),
      };
    }
  }

  for (const file of filesArr ?? []) {
    total_count += 1;
    total_disk += toNum(file?.size);
    total_size += toNum(file?.size);
    if (file.accessed > accessed) accessed = file.accessed;
    if (file.modified > modified) modified = file.modified;

    const owner = file.owner;
    if (owner) {
      const prev = aggUsers[owner] ?? {
        username: owner,
        count: 0,
        disk: 0,
        size: 0,
        linked: 0,
        atime: 0,
        mtime: 0,
      };
      aggUsers[owner] = {
        username: owner,
        count: prev.count + 1,
        disk: prev.disk + toNum(file.size),
        size: prev.size + toNum(file.size),
        linked: prev.linked,
        atime: Math.max(prev.atime, toNum(file.accessed)),
        mtime: Math.max(prev.mtime, toNum(file.modified)),
      };
    }
  }

  return { path: p, total_count, total_disk, total_size, total_linked, accessed, modified, users: aggUsers };
}

export function safeNumber(v: any) {
  return toNum(v);
}
