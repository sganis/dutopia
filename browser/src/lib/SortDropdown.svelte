<!-- browser/src/lib/SortDropdown.svelte -->
<script lang="ts">
  type SortKey = "disk" | "size" | "count";

  let {
    sortBy = $bindable("disk"),
  }: {
    sortBy: SortKey;
  } = $props();

  let sortOpen = $state(false);

  function displaySortBy(key: string) {
    switch (key) {
      case "disk":
        return "Disk Usage";
      case "size":
        return "File Size";
      case "count":
        return "Total Files";
      default:
        return "Disk Usage";
    }
  }

  function chooseSort(key: SortKey) {
    sortBy = key;
    sortOpen = false;
  }

  function clickOutside(node: HTMLElement, cb: () => void) {
    const onPointerDown = (e: PointerEvent) => {
      if (!node.contains(e.target as Node)) cb();
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape" || e.key === "Esc") cb();
    };

    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown, true);

    return {
      destroy() {
        document.removeEventListener("pointerdown", onPointerDown, true);
        document.removeEventListener("keydown", onKeyDown, true);
      },
    };
  }
</script>

<div class="relative" use:clickOutside={() => (sortOpen = false)}>
  <button class="btn w-36" onclick={() => (sortOpen = !sortOpen)}>
    <div class="flex items-center gap-2">
      <span class="material-symbols-outlined">sort</span>
      {displaySortBy(sortBy)}
    </div>
  </button>
  {#if sortOpen}
    <div
      class="flex flex-col divide-y divide-gray-500 absolute w-36 rounded border
       border-gray-500 bg-gray-800 shadow-lg z-20 overflow-hidden mt-0.5"
    >
      <button
        class="w-full text-left px-3 py-2 hover:bg-gray-700 text-nowrap"
        onclick={() => chooseSort("disk")}
      >
        By Disk Usage
      </button>
      <button
        class="w-full text-left px-3 py-2 hover:bg-gray-700 text-nowrap"
        onclick={() => chooseSort("size")}
      >
        By File Size
      </button>
      <button class="w-full text-left px-3 py-2 hover:bg-gray-700" onclick={() => chooseSort("count")}>
        By Total Files
      </button>
    </div>
  {/if}
</div>
