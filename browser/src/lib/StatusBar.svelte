<!-- browser/src/lib/StatusBar.svelte -->
<!--
  Bottom status bar. Shows scan progress when a scan is running, and the last
  error if any. Desktop-only (renders nothing in web builds).
-->
<script lang="ts">
  import { scanStatus } from "../ts/scan.svelte";
  import { api } from "$api";

  const stageLabel = (s: string) =>
    s === "scan"
      ? "Scanning filesystem"
      : s === "sum"
      ? "Summarizing"
      : s === "index"
      ? "Indexing"
      : "";

  async function cancel() {
    try { await api.cancelScan(); } catch { /* ignore */ }
  }
</script>

{#if scanStatus.running || scanStatus.busy || scanStatus.error}
  <div class="fixed bottom-0 inset-x-0 z-40 flex items-center gap-3 px-3 py-1 bg-gray-900 border-t border-gray-700 text-xs text-gray-200 select-none">
    {#if scanStatus.running}
      <span
        class="inline-block w-3 h-3 rounded-full border-2 border-gray-600 border-t-orange-500 animate-spin"
        aria-label="Scanning"
      ></span>
      <span class="text-gray-300 whitespace-nowrap">{stageLabel(scanStatus.stage)}</span>
      <span class="text-gray-400 truncate flex-1" title={scanStatus.message}>{scanStatus.message}</span>
      <button
        class="px-2 py-0.5 rounded bg-gray-700 hover:bg-gray-600 text-gray-100"
        onclick={cancel}
        title="Cancel scan"
      >
        Cancel
      </button>
    {:else if scanStatus.busy}
      <span class="inline-block w-3 h-3 rounded-full border-2 border-gray-600 border-t-orange-500 animate-spin"></span>
      <span class="text-gray-300 truncate flex-1" title={scanStatus.busyLabel}>{scanStatus.busyLabel}</span>
    {:else if scanStatus.error}
      <span class="material-symbols-outlined text-red-400 text-base">error</span>
      <span class="text-red-300 truncate flex-1" title={scanStatus.error}>{scanStatus.error}</span>
      <button
        class="px-2 py-0.5 rounded bg-gray-700 hover:bg-gray-600 text-gray-100"
        onclick={() => (scanStatus.error = "")}
      >
        Dismiss
      </button>
    {/if}
  </div>
{/if}
