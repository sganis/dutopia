<!-- browser/src/lib/FolderBar.svelte -->
<script lang="ts">
  import type { SvelteMap } from "svelte/reactivity";
  import { humanBytes, humanCount, humanTime, isWindows } from "../ts/util";
  import ActionBar from "./ActionBar.svelte";

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
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="relative px-2 py-1 cursor-pointer hover:opacity-95 bg-gray-700 border border-gray-500 rounded-lg overflow-hidden min-h-16"
  onclick={onclick}
>
  <!-- Folder bar background -->
  <div class="absolute left-0 top-0 bottom-0 flex z-0" style="width: {widthPercent}%">
    {#each sortedUserEntries(folder) as [uname, userData]}
      {@const userMetric = sortBy === "disk" ? userData.disk : sortBy === "size" ? userData.size : userData.count}
      {@const totalMetric =
        sortBy === "disk" ? folder.total_disk : sortBy === "size" ? folder.total_size : folder.total_count}
      {@const userPercent = totalMetric > 0 ? (userMetric / totalMetric) * 100 : 0}
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
  <!-- Folder bar foreground -->
  <div class="relative flex flex-col gap-2 z-10 pointer-events-none">
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
        onDelete={onDelete && singleOwner
          ? () => onDelete(folder.path, toNum(folder.total_disk), singleOwner!)
          : undefined}
      />
      <p class="text-sm text-right">
        {bottomValueFolder(folder)}
        • Updated {humanTime(folder.modified)}
        {#if folder.accessed > folder.modified}
          • Last file read {humanTime(folder.accessed)}
        {/if}
      </p>
    </div>
  </div>
</div>
