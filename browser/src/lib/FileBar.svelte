<!-- browser/src/lib/FileBar.svelte -->
<script lang="ts">
  import type { SvelteMap } from "svelte/reactivity";
  import { humanBytes, humanTime } from "../ts/util";

  type ScannedFile = {
    path: string;
    size: number;
    accessed: number;
    modified: number;
    owner: string;
  };

  type SortKey = "disk" | "size" | "count";

  let {
    file,
    sortBy,
    widthPercent,
    userColors,
    onCopyPath,
  }: {
    file: ScannedFile;
    sortBy: SortKey;
    widthPercent: number;
    userColors: SvelteMap<string, string>;
    onCopyPath?: (e: MouseEvent) => void;
  } = $props();

  const toNum = (v: any) => {
    const n = Number(v);
    return Number.isFinite(n) ? n : 0;
  };

  function rightValueFile(f: ScannedFile) {
    switch (sortBy) {
      case "disk":
      case "size":
        return humanBytes(toNum(f.size));
      case "count":
        return "1";
    }
  }

  const color = $derived(userColors.get(file.owner));
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="flex">
  <span class="material-symbols-outlined text-4xl">subdirectory_arrow_right</span>
  <div class="relative flex grow px-2 py-1 bg-gray-700 border border-gray-500 rounded overflow-hidden text-xs">
    <div class="flex flex-col w-full">
      <!-- File bar background -->
      <div
        class="absolute left-0 top-0 bottom-0 z-0 opacity-60"
        style="width: {widthPercent}%; background-color: {color};"
      ></div>
      <div class="relative z-10 flex items-center justify-between gap-2">
        <div class="w-full overflow-hidden">
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <span class="cursor-pointer text-ellipsis text-nowrap" onclick={onCopyPath}>
            {file.path}
          </span>
        </div>
        <div class="flex items-center gap-4 text-sm font-semibold text-nowrap">
          {rightValueFile(file)}
        </div>
      </div>
      <div class="relative z-10 flex justify-between">
        <div class="">{file.owner}</div>
        <div class="">
          Updated {humanTime(file.modified)}
          {#if humanTime(file.accessed) > humanTime(file.modified)}
            • Read {humanTime(file.accessed)}
          {/if}
        </div>
      </div>
    </div>
  </div>
</div>
