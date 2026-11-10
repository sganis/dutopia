<!-- browser/src/lib/FolderBar.svelte -->
<script lang="ts">
  import type { SvelteMap } from "svelte/reactivity";
  import { humanBytes, humanCount, humanTime, isWindows } from "../ts/util";
  import { groupSmallUsers, MANY_USERS } from "../ts/manyusers";
  import { api } from "$api";
  import ActionBar from "./ActionBar.svelte";
  import type { TrashAction } from "../ts/models";

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
    folder,
    sortBy,
    widthPercent,
    userColors,
    onclick,
    onCopyPath,
    onReveal,
    onTerminal,
    onDelete,
    onArchive,
    canDelete,
    queued = null,
    onUserHover,
    onUserMove,
    onUserLeave,
  }: {
    folder: FolderItem;
    sortBy: SortKey;
    widthPercent: number;
    userColors: SvelteMap<string, string>;
    onclick: () => void;
    onCopyPath?: (path: string) => void;
    onReveal?: (path: string) => void;
    onTerminal?: (path: string) => void;
    /** Called when the user clicks the trash icon. `owner` is set when the
     *  folder has exactly one owner (web build requires single-owner folders
     *  for cleanup requests). Desktop passes whatever owner — it ignores it. */
    onDelete?: (path: string, size: number, owner: string) => void;
    /** The archive icon (web only); same arguments and gate as onDelete. */
    onArchive?: (path: string, size: number, owner: string) => void;
    /** Extra gate on the trash and archive icons (web: the trash tiers +
     *  FILESYSTEMS). Unset = shown for every single-owner folder. */
    canDelete?: (path: string, owner: string) => boolean;
    /** Already in the cart: that icon shows it and toggles Keep. */
    queued?: TrashAction | null;
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

  function sortedUserEntries(f: FolderItem) {
    return Object.entries(f?.users ?? {}).sort(([, a], [, b]) => userMetricFor(a) - userMetricFor(b));
  }

  function rightValueFolder(f: FolderItem) {
    switch (sortBy) {
      case "disk":
        return humanBytes(toNum(f?.total_disk));
      case "size":
        return humanBytes(toNum(f?.total_size));
      case "count":
        return `Files: ${humanCount(toNum(f?.total_count))}`;
    }
  }

  function bottomValueFolder(f: FolderItem) {
    const linkedSuffix = isWindows ? "" : ` • Linked: ${humanBytes(toNum(f?.total_linked))}`;
    switch (sortBy) {
      case "disk":
      case "size":
        return `${humanCount(toNum(f?.total_count))} Files${linkedSuffix}`;
      case "count":
        return `Disk: ${humanBytes(toNum(f?.total_disk))}${linkedSuffix}`;
    }
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

  // A folder can be queued for cleanup only when every contributing file is
  // owned by the same user — mixed-owner folders have no single target for
  // the request. Hide the delete icon in that case rather than disabling it
  // (the row's other actions still work).
  const singleOwner = $derived.by(() => {
    const keys = Object.keys(folder.users ?? {});
    return keys.length === 1 ? keys[0] : null;
  });
  const allowed = $derived(!!singleOwner && (canDelete?.(folder.path, singleOwner) ?? true));

  // Users too thin to see merge into one light "Many Users" segment (first,
  // as the smallest); `lightW` is its width in % of the row, where the text
  // turns dark.
  let rowW = $state(0);
  const segments = $derived.by(() => {
    const total = toNum(
      sortBy === "disk" ? folder.total_disk : sortBy === "size" ? folder.total_size : folder.total_count,
    );
    const barPx = (rowW * widthPercent) / 100;
    return groupSmallUsers(sortedUserEntries(folder), userMetricFor, total, barPx, api.manyUsersPx).map(([uname, userData]) => ({
      uname,
      userData,
      userPercent: total > 0 ? (userMetricFor(userData) / total) * 100 : 0,
    }));
  });
  const lightW = $derived(segments[0]?.uname === MANY_USERS ? (widthPercent * segments[0].userPercent) / 100 : 0);
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="relative px-2 py-1 cursor-pointer hover:opacity-95 rounded-lg overflow-hidden min-h-16"
  style="background-color: {api.barEmptyColor}"
  onclick={onclick}
  bind:clientWidth={rowW}
>
  <!-- Folder bar background -->
  <div class="absolute left-0 top-0 bottom-0 flex z-0" style="width: {widthPercent}%">
    {#each segments as { uname, userData, userPercent } (uname)}
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
    <div class="flex items-center justify-between gap-4">
      <div class="w-full overflow-hidden text-ellipsis whitespace-nowrap">
        <div>{folder.path}</div>
      </div>
      <span class="text-nowrap font-bold">{rightValueFolder(folder)}</span>
    </div>
    <div class="flex items-center justify-between gap-2">
      <ActionBar
        onCopy={onCopyPath ? () => onCopyPath(folder.path) : undefined}
        onReveal={onReveal ? () => onReveal(folder.path) : undefined}
        onTerminal={onTerminal ? () => onTerminal(folder.path) : undefined}
        onDelete={onDelete && allowed ? () => onDelete(folder.path, toNum(folder.total_disk), singleOwner!) : undefined}
        deleteActive={queued === "trash"}
        onArchive={onArchive && allowed ? () => onArchive(folder.path, toNum(folder.total_disk), singleOwner!) : undefined}
        archiveActive={queued === "archive"}
      />
      <p class="text-sm text-right">
        {bottomValueFolder(folder)}
        • Updated {humanTime(folder.modified)}
        {#if folder.accessed > folder.modified}
          • Last file read {humanTime(folder.accessed)}
        {/if}
      </p>
    </div>
  {/snippet}
  <!-- Folder bar foreground. Clipped to exclude the light zone: the dark
       copy there must sit directly on the bone color, with no white text
       or its halo bleeding underneath (the halo blurs dark text). -->
  <div
    class="absolute inset-0 px-2 py-1 flex flex-col gap-2 z-10 pointer-events-none bar-text"
    style={lightW > 0 ? `clip-path: inset(0 0 0 ${lightW}%)` : undefined}
  >
    {@render content()}
  </div>
  <!-- Dark copy of the foreground, clipped to the light "Many Users" zone -->
  {#if lightW > 0}
    <div
      class="absolute inset-0 px-2 py-1 flex flex-col gap-2 z-20 pointer-events-none text-gray-900"
      style="clip-path: inset(0 {100 - lightW}% 0 0)"
      inert
      aria-hidden="true"
    >
      {@render content()}
    </div>
  {/if}
  <!-- Rim drawn on top and translucent (not a border): a light segment
       reaches the edge instead of looking inset by a dark frame. -->
  <div class="absolute inset-0 rounded-lg ring-1 ring-inset ring-white/25 z-30 pointer-events-none"></div>
</div>
