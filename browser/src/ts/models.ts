// browser/src/ts/models.ts

export type Age = {
  count: number;
  disk: number;
  size: number;
  linked: number;
  atime: number;
  mtime: number;
};

export type RawFolder = {
  path: string;
  users: Record<string, Record<string, Age>>;
};

export type UserStatsJson = {
  username: string;
  count: number;
  disk: number;
  size: number;
  linked: number;
  atime: number;
  mtime: number;
};

export type FolderItem = {
  path: string;
  total_count: number;
  total_disk: number;
  total_size: number;
  total_linked: number;
  accessed: number;
  modified: number;
  users: Record<string, UserStatsJson>;
};

export type ScannedFile = {
  path: string;
  size: number;
  accessed: number;
  modified: number;
  owner: string;
};

/** One owner's files directly in a folder, listed or not. */
export type FileTotals = {
  count: number;
  size: number;
  accessed: number;
  modified: number;
};

/** /api/files: a folder's largest files, plus totals of all its files per
 *  owner (the list can be short; the totals never are). */
export type FilesResult = {
  files: ScannedFile[];
  totals: Record<string, FileTotals>;
};

export type SortKey = "disk" | "size" | "count";
export type AgeFilterType = -1 | 0 | 1 | 2;

/** What the cron does with a queued item: move it to `<fs>/.trash`, or pack
 *  it into `<fs>/.archive` as .tar.gz. */
export type TrashAction = "trash" | "archive";

/** One queued item of the server-side trash (src/bin/duapi/trash.rs). */
export type TrashEntry = {
  path: string;
  size: number;
  owner: string;
  actor: string;
  fs: string;
  /** Unix seconds. */
  queued_at: number;
  /** Missing from an older duapi: trash. */
  action?: TrashAction;
};
