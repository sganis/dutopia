<!-- browser/src/lib/PathStats.svelte -->
<script lang="ts">
  import type { SvelteMap } from "svelte/reactivity";
  import { humanBytes, humanCount, humanTime } from "../ts/util";
  import { groupSmallUsers, MANY_USERS } from "../ts/manyusers";
  import { api } from "$api";

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

  // Users too thin to see merge into one light "Many Users" segment (first,
  // as the smallest); the text over it turns dark.
  let rowW = $state(0);
  const segments = $derived.by(() => {
    const total = toNum(
      sortBy === "disk" ? pathTotals.total_disk : sortBy === "size" ? pathTotals.total_size : pathTotals.total_count,
    );
    return groupSmallUsers(sortedUserEntries(pathTotals), userMetricFor, total, rowW, api.manyUsersPx).map(([uname, userData]) => ({
      uname,
      userData,
      userPercent: total > 0 ? (userMetricFor(userData) / total) * 100 : 0,
    }));
  });
  const lightW = $derived(segments[0]?.uname === MANY_USERS ? segments[0].userPercent : 0);

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

<div class="relative px-2 rounded overflow-hidden text-sm p-1 h-8 shrink-0" style="background-color: {api.barEmptyColor}" bind:clientWidth={rowW}>
  <!-- Total bar background -->
  <div class="flex absolute left-0 top-0 bottom-0 z-0" style="width: 100%">
    {#each segments as { uname, userData, userPercent } (uname)}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="h-full transition-all duration-300 min-w-[0.5px] hover:opacity-90 border-r border-r-gray-600 last:border-r-0"
        style="width: {userPercent}%; background-color: {uname === MANY_USERS ? api.manyUsersColor : userColors.get(uname)};"
        onmouseenter={(e) => onUserHover?.(e, userData, userPercent)}
        onmousemove={onUserMove}
        onmouseleave={onUserLeave}
        aria-label={`${userData.username}: ${rightValueUser(userData)}`}
      ></div>
    {/each}
  </div>
  {#snippet content()}
    <div class="flex items-center justify-end whitespace-nowrap">
      {humanCount(pathTotals.total_count)} Files
      • Changed {humanTime(pathTotals.modified)}
      • {humanBytes(pathTotals.total_disk)}
    </div>
  {/snippet}
  <!-- Total bar foreground. Clipped to exclude the light zone: see FolderBar. -->
  <div
    class="absolute inset-0 px-2 p-1 z-10 pointer-events-none bar-text"
    style={lightW > 0 ? `clip-path: inset(0 0 0 ${lightW}%)` : undefined}
  >
    {@render content()}
  </div>
  <!-- Dark copy of the foreground, clipped to the light "Many Users" zone -->
  {#if lightW > 0}
    <div
      class="absolute inset-0 px-2 p-1 z-20 pointer-events-none text-gray-900"
      style="clip-path: inset(0 {100 - lightW}% 0 0)"
      inert
      aria-hidden="true"
    >
      {@render content()}
    </div>
  {/if}
  <!-- Rim on top, translucent: see FolderBar. -->
  <div class="absolute inset-0 rounded ring-1 ring-inset ring-white/25 z-30 pointer-events-none"></div>
</div>
