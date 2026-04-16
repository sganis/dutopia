<!-- browser/src/lib/PageContent.svelte -->
<script lang="ts">
  import type { SvelteMap } from "svelte/reactivity";
  import FolderBar from "./FolderBar.svelte";
  import FileBar from "./FileBar.svelte";

  export let initializing: boolean;
  export let loading: boolean;
  export let progress_percent: number;
  export let progress_current: number;
  export let progress_total: number;
  export let sortedFolders: any[];
  export let sortedfiles: any[];
  export let sortBy: "disk" | "size" | "count";
  export let userColors: SvelteMap<string, string>;
  export let pct: (folder: any) => number;
  export let filePct: (file: any) => number;
  export let onFolderClick: (path: string) => void;
  export let onCopyPath: (e: MouseEvent) => void;
  export let onUserHover: (e: MouseEvent, userData: any, percent: number) => void;
  export let onUserMove: (e: MouseEvent) => void;
  export let onUserLeave: () => void;
</script>

{#if initializing}
  <div class="flex flex-col w-full h-full items-center justify-between font-mono">
    <div class="w-full bg-gray-700 rounded-full h-1">
      <div class="bg-orange-500 h-1 rounded-full transition-all duration-300" style="width: {progress_percent}%"></div>
    </div>
    <div class="flex flex-col justify-center grow items-center w-64">
      <div class="flex w-full justify-between">
        <div>Progress:</div>
        <div>{progress_percent}%</div>
      </div>
      <div class="flex w-full justify-between">
        <div>Loaded folders:</div>
        <div>{progress_current}</div>
      </div>
      <div class="flex w-full justify-between">
        <div>Total:</div>
        <div>{progress_total}</div>
      </div>
    </div>
  </div>
{:else if loading}
  <div class="flex flex-col gap-2 overflow-y-auto">
    {#each Array(6) as _, i}
      <div class="relative p-3 bg-gray-800 border border-gray-500 rounded-lg animate-pulse min-h-16 h-16">
        <div class="flex items-center justify-between gap-4">
          <div class="h-4 bg-gray-700 rounded w-3/4"></div>
          <div class="h-3 bg-gray-700 rounded w-12"></div>
        </div>
        <div class="flex gap-2 mt-2">
          <div class="h-3 bg-gray-700 rounded w-16"></div>
          <div class="h-3 bg-gray-700 rounded w-20"></div>
          <div class="h-3 bg-gray-700 rounded w-24"></div>
        </div>
      </div>
    {/each}
  </div>
{:else}
  <div class="flex flex-col gap-2 overflow-y-auto transition-opacity duration-200 p-4">
    {#each sortedFolders as folder}
      <FolderBar
        {folder}
        {sortBy}
        widthPercent={pct(folder)}
        {userColors}
        onclick={() => onFolderClick(folder.path)}
        onUserHover={onUserHover}
        onUserMove={onUserMove}
        onUserLeave={onUserLeave}
      />
    {/each}

    {#each sortedfiles as file}
      <FileBar {file} {sortBy} widthPercent={filePct(file)} {userColors} onCopyPath={onCopyPath} />
    {/each}
  </div>
{/if}
