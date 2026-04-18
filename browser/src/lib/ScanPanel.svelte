<!-- browser/src/lib/ScanPanel.svelte -->
<!--
  Desktop-only scan button + picker modal. Progress rendering lives in
  StatusBar.svelte at the bottom of the window; this component only owns
  the trigger button, the folder-selection modal, and the scan flow.

  Opens a picker with up to 5 recent folders (MRU). User can add arbitrary
  folders via the native folder dialog. Selected paths go to `api.scan()`
  which runs duscan → dusum → dudb and replaces the DB.
-->
<script lang="ts">
  import { onDestroy } from "svelte";
  import { api } from "$api";
  import type { ScanProgress } from "$api";
  import { scanStatus, resetScanStatus } from "../ts/scan.svelte";

  type Row = { path: string; recent: boolean };

  let {
    onComplete,
  }: {
    onComplete?: (scannedPaths: string[]) => void;
  } = $props();

  let pickerOpen = $state(false);
  let rows = $state<Row[]>([]);
  let selected = $state<Record<string, boolean>>({});
  let unlisten: (() => void) | null = null;

  async function openPicker() {
    resetScanStatus();
    let recent: string[] = [];
    try {
      recent = await api.getRecentPaths();
    } catch (e: any) {
      scanStatus.error = e?.message ?? String(e);
      return;
    }

    rows = recent.map((p) => ({ path: p, recent: true }));
    selected = {};
    for (const p of recent) selected[p] = true;
    pickerOpen = true;
  }

  async function addFolder() {
    try {
      const mod = await import("@tauri-apps/plugin-dialog");
      const picked = await mod.open({ directory: true, multiple: false });
      if (typeof picked !== "string" || picked.length === 0) return;
      if (!rows.some((r) => r.path === picked)) {
        rows = [...rows, { path: picked, recent: false }];
      }
      selected[picked] = true;
    } catch (e: any) {
      scanStatus.error = e?.message ?? String(e);
    }
  }

  async function remove(path: string) {
    rows = rows.filter((r) => r.path !== path);
    delete selected[path];
    selected = { ...selected };
    // Persist immediately so the path doesn't reappear on the next picker
    // open. Only pass the rows currently shown — anything else was already
    // pruned in prior sessions.
    await api.setRecentPaths(rows.map((r) => r.path));
  }

  function toggle(path: string) {
    selected[path] = !selected[path];
  }

  async function confirmScan() {
    const paths = rows.filter((r) => selected[r.path]).map((r) => r.path);
    if (paths.length === 0) return;
    pickerOpen = false;

    scanStatus.running = true;
    scanStatus.stage = "";
    scanStatus.percent = 0;
    scanStatus.message = "Starting…";
    scanStatus.error = "";

    try {
      unlisten = await api.onScanProgress((p: ScanProgress) => {
        scanStatus.stage = p.stage;
        scanStatus.percent = p.percent;
        scanStatus.message = p.message ?? "";
      });
      await api.scan(paths);
      onComplete?.(paths);
    } catch (e: any) {
      scanStatus.error = e?.message ?? String(e);
    } finally {
      scanStatus.running = false;
      scanStatus.stage = "";
      scanStatus.percent = 0;
      scanStatus.message = "";
      if (unlisten) {
        unlisten();
        unlisten = null;
      }
    }
  }

  onDestroy(() => {
    if (unlisten) unlisten();
  });
</script>

<button class="btn" onclick={openPicker} title="Scan filesystem" disabled={scanStatus.running}>
  <div class="flex items-center gap-1">
    <span class="material-symbols-outlined">travel_explore</span>
    <span>Scan</span>
  </div>
</button>

{#if pickerOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
    onclick={() => (pickerOpen = false)}
  >
    <div
      class="bg-gray-800 border border-gray-500 rounded-lg shadow-lg p-5 min-w-96 max-w-lg w-full"
      onclick={(e) => e.stopPropagation()}
    >
      <div class="text-lg font-semibold mb-1 text-gray-100">Scan folders</div>
      <div class="text-xs text-gray-400 mb-3">
        A new scan replaces the current index.
      </div>
      <div class="flex flex-col gap-2 max-h-80 overflow-y-auto mb-3">
        {#each rows as r}
          <label class="flex items-center gap-2 p-2 rounded border border-gray-600 hover:bg-gray-700 cursor-pointer">
            <input type="checkbox" checked={selected[r.path]} onchange={() => toggle(r.path)} />
            <div class="flex flex-col flex-1 min-w-0">
              <span class="text-gray-100 text-sm truncate" title={r.path}>{r.path}</span>
              <span class="text-xs text-gray-400">{r.recent ? "Recent" : "New"}</span>
            </div>
            <button
              type="button"
              class="text-gray-400 hover:text-red-400 rounded p-0.5"
              title="Remove"
              aria-label="Remove"
              onclick={(e) => { e.preventDefault(); remove(r.path); }}
            >
              <span class="material-symbols-outlined text-base">close</span>
            </button>
          </label>
        {:else}
          <div class="text-sm text-gray-400 text-center py-4">
            No recent folders. Click "Add folder…" to pick one.
          </div>
        {/each}
      </div>
      <div class="flex justify-between items-center gap-2">
        <button
          class="px-3 py-1 rounded border border-gray-500 bg-gray-700 hover:bg-gray-600 text-gray-100 text-sm"
          onclick={addFolder}
        >
          <span class="inline-flex items-center gap-1">
            <span class="material-symbols-outlined text-base">add</span>
            Add folder…
          </span>
        </button>
        <div class="flex gap-2">
          <button class="px-3 py-1 rounded bg-gray-700 hover:bg-gray-600 text-gray-100" onclick={() => (pickerOpen = false)}>Cancel</button>
          <button
            class="px-3 py-1 rounded bg-emerald-600 hover:bg-emerald-500 text-white disabled:opacity-50"
            onclick={confirmScan}
            disabled={rows.filter((r) => selected[r.path]).length === 0}
          >
            Start scan
          </button>
        </div>
      </div>
    </div>
  </div>
{/if}
