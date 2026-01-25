<!-- browser/src/lib/AgeFilter.svelte -->
<script lang="ts">
  type AgeFilter = -1 | 0 | 1 | 2;

  let {
    ageFilter = $bindable(-1),
    onchange,
  }: {
    ageFilter: AgeFilter;
    onchange?: () => void;
  } = $props();

  let ageOpen = $state(false);

  const AGE_LABELS: Record<AgeFilter, string> = {
    "-1": "Any Time",
    0: "Recent",
    1: "Not too old",
    2: "Old files",
  };

  function displayAgeLabel(a: AgeFilter) {
    return AGE_LABELS[a];
  }

  function chooseAge(a: AgeFilter) {
    ageFilter = a;
    ageOpen = false;
    onchange?.();
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

<div class="relative" use:clickOutside={() => (ageOpen = false)}>
  <button class="btn w-36" onclick={() => (ageOpen = !ageOpen)}>
    <div class="flex items-center gap-2">
      <span class="material-symbols-outlined">schedule</span>
      {displayAgeLabel(ageFilter)}
    </div>
  </button>
  {#if ageOpen}
    <div
      class="flex flex-col divide-y divide-gray-500 absolute w-48 rounded border
       border-gray-500 bg-gray-800 shadow-lg z-20 overflow-hidden mt-0.5"
    >
      <button class="w-full text-left px-3 py-2 hover:bg-gray-700" onclick={() => chooseAge(-1)}>
        Any Time
      </button>
      <button class="w-full text-left px-3 py-2 hover:bg-gray-700" onclick={() => chooseAge(0)}>
        Recent (2 months)
      </button>
      <button class="w-full text-left px-3 py-2 hover:bg-gray-700" onclick={() => chooseAge(1)}>
        Not too old (2 years)
      </button>
      <button class="w-full text-left px-3 py-2 hover:bg-gray-700" onclick={() => chooseAge(2)}>
        Old files
      </button>
    </div>
  {/if}
</div>
