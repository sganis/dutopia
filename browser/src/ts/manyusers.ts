// browser/src/ts/manyusers.ts
// Users too thin to see in a bar merge into one "Many Users" segment. All
// the colors summed up are white, so it is painted bone white (the empty
// bar stays dark), and the text over it turns dark.
import type { UserStatsJson } from "./models";

/** Segments narrower than this many pixels merge into "Many Users". */
export const MANY_PX = 2;
export const MANY_USERS = "Many Users";
export const MANY_COLOR = "#D2CCBC";

/** Merges the users whose segment would be under MANY_PX in a bar `barPx`
 *  wide into one MANY_USERS entry, placed first (bars sort users smallest
 *  first). Returns `entries` as is when fewer than two would merge or the
 *  bar is not measured yet. */
export function groupSmallUsers(
  entries: [string, UserStatsJson][],
  metric: (u: UserStatsJson) => number,
  total: number,
  barPx: number,
): [string, UserStatsJson][] {
  if (barPx <= 0 || total <= 0) return entries;
  const cutoff = (MANY_PX * total) / barPx;
  const small = entries.filter(([, u]) => metric(u) < cutoff);
  if (small.length < 2) return entries;

  const many: UserStatsJson = {
    username: MANY_USERS, count: 0, disk: 0, size: 0, linked: 0, atime: 0, mtime: 0,
  };
  for (const [, u] of small) {
    many.count += Number(u.count) || 0;
    many.disk += Number(u.disk) || 0;
    many.size += Number(u.size) || 0;
    many.linked += Number(u.linked) || 0;
    many.atime = Math.max(many.atime, Number(u.atime) || 0);
    many.mtime = Math.max(many.mtime, Number(u.mtime) || 0);
  }
  return [[MANY_USERS, many], ...entries.filter(([, u]) => metric(u) >= cutoff)];
}
