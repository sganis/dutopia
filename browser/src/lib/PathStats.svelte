<!-- browser/src/lib/PathStats.svelte -->
<script lang="ts">
  import type { SvelteMap } from "svelte/reactivity";
  import { humanBytes, humanCount, humanTime } from "../ts/util";

  type UserStatsJson = {
    username: string;
    count: number;
    disk: number;
    size: number;
    linked: number;
    atime: number;
    mtime: number;
  };

  type FolderItem = {
    path: string;
    total_count: number;
    total_disk: number;
    total_size: number;
    total_linked: number;
    accessed: number;
    modified: number;
    users: Record<string, UserStatsJson>;
  };

  type SortKey = "disk" | "size" | "count";

  let {
    pathTotals,
    sortBy,
    userColors,
    onUserHover,
    onUserMove,
    onUserLeave,
  }: {
    pathTotals: FolderItem;
    sortBy: SortKey;
    userColors: SvelteMap<string, string>;
    onUserHover?: (e: MouseEvent, userData: UserStatsJson, percent: number) => void;
    onUserMove?: (e: MouseEvent) => void;
    onUserLeave?: () => void;
  } = $props();

  const toNum = (v: any) => {
    const n = Number(v);
    return Number.isFinite(n) ? n : 0;
  };

  const userMetricFor = (ud: UserStatsJson) => {
    switch (sortBy) {
      case "disk":
        return Number(ud.disk);
      case "size":
        return Number(ud.size);
      case "count":
        return Number(ud.count);
    }
  };

  function sortedUserEntries(file: FolderItem) {
    return Object.entries(file?.users ?? {}).sort(([, a], [, b]) => userMetricFor(a) - userMetricFor(b));
  }

  function rightValueUser(userData: UserStatsJson) {
    switch (sortBy) {
      case "disk":
        return humanBytes(toNum(userData?.disk));
      case "size":
        return humanBytes(toNum(userData?.size));
      case "count":
        return humanCount(toNum(userData?.count));
    }
  }
</script>

<div class="relative px-2 bg-gray-700 border border-gray-500 rounded text-sm p-1">
  <!-- Total bar background -->
  <div class="flex absolute left-0 top-0 bottom-0 z-0" style="width: 100%">
    {#each sortedUserEntries(pathTotals) as [uname, userData] (uname)}
      {@const userMetric = sortBy === "disk" ? userData.disk : sortBy === "size" ? userData.size : userData.count}
      {@const totalMetric =
        sortBy === "disk" ? pathTotals.total_disk : sortBy === "size" ? pathTotals.total_size : pathTotals.total_count}
      {@const userPercent = totalMetric > 0 ? (userMetric / totalMetric) * 100 : 0}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="h-full transition-all duration-300 min-w-[0.5px] hover:opacity-90"
        style="width: {userPercent}%; background-color: {userColors.get(uname)};"
        onmouseenter={(e) => onUserHover?.(e, userData, userPercent)}
        onmousemove={onUserMove}
        onmouseleave={onUserLeave}
        aria-label={`${userData.username}: ${rightValueUser(userData)}`}
      ></div>
    {/each}
  </div>
  <!-- Total bar foreground -->
  <div class="relative z-10 pointer-events-none">
    <div class="flex items-center justify-end">
      {humanCount(pathTotals.total_count)} Files
      • Changed {humanTime(pathTotals.modified)}
      • {humanBytes(pathTotals.total_disk)}
    </div>
  </div>
</div>
