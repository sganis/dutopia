<!-- browser/src/lib/DeletePanel.svelte -->
<!--
  Deletion queue panel. Lists every path the user has targeted for deletion.
  Per-row Delete and Keep buttons; Delete all at the top runs all queued
  items in parallel. Each item shows a spinner while its trash call is in
  flight, or an error state if the call failed.
-->
<script lang="ts">
  import { deleteQueue, removeFromQueue, deleteOne, deleteAll } from "../ts/deleteQueue.svelte";
  import { humanBytes } from "../ts/util";

  let {
    onChange,
  }: {
    /** Fires after a successful delete so the parent can refresh the view. */
    onChange?: () => void;
  } = $props();

  async function clickDelete(path: string) {
    await deleteOne(path);
    onChange?.();
  }

  async function clickDeleteAll() {
    await deleteAll();
    onChange?.();
  }

  async function close() {
    deleteQueue.panelOpen = false;
  }

  const anyDeleting = $derived(deleteQueue.items.some((i) => i.status === "deleting"));
  const deletableCount = $derived(deleteQueue.items.filter((i) => i.status !== "deleting").length);
  const totalSize = $derived(deleteQueue.items.reduce((a, i) => a + (i.size || 0), 0));
</script>

{#if deleteQueue.panelOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
    onclick={close}
  >
    <div
      class="bg-gray-800 border border-gray-500 rounded-lg shadow-lg p-5 min-w-[32rem] max-w-2xl w-full h-[80vh] flex flex-col"
      onclick={(e) => e.stopPropagation()}
    >
      <div class="flex items-center justify-between mb-3">
        <div>
          <div class="text-lg font-semibold text-gray-100">Deletion queue</div>
          <div class="text-xs text-gray-400">
            {deleteQueue.items.length} item{deleteQueue.items.length === 1 ? "" : "s"}
            &nbsp;•&nbsp; total <span class="text-gray-200 font-medium">{humanBytes(totalSize)}</span>
            &nbsp;•&nbsp; moved to OS trash
          </div>
        </div>
        <button
          class="px-3 py-1 rounded bg-red-600 hover:bg-red-500 disabled:opacity-50 text-white text-sm"
          onclick={clickDeleteAll}
          disabled={deletableCount === 0 || anyDeleting}
          title="Delete all queued items in parallel"
        >
          Delete all
        </button>
      </div>

      <div class="flex flex-col gap-1 overflow-y-auto flex-1 border border-gray-700 rounded bg-gray-900/40 p-1">
        {#each deleteQueue.items as item (item.path)}
          <div class="flex items-center gap-2 p-2 rounded hover:bg-gray-700/40">
            <div class="flex-1 min-w-0">
              <div class="text-sm text-gray-100 truncate" title={item.path}>{item.path}</div>
              <div class="text-xs text-gray-400">
                {humanBytes(item.size || 0)}
                {#if item.status === "error" && item.error}
                  &nbsp;•&nbsp; <span class="text-red-400" title={item.error}>{item.error}</span>
                {/if}
              </div>
            </div>

            {#if item.status === "deleting"}
              <span
                class="inline-block w-4 h-4 rounded-full border-2 border-gray-600 border-t-orange-500 animate-spin"
                aria-label="Deleting"
                title="Deleting…"
              ></span>
            {/if}

            <button
              class="px-2 py-0.5 rounded bg-gray-700 hover:bg-gray-600 text-gray-100 text-xs disabled:opacity-50"
              onclick={() => removeFromQueue(item.path)}
              disabled={item.status === "deleting"}
              title="Remove from queue (do not delete)"
            >
              Keep
            </button>
            <button
              class="px-2 py-0.5 rounded bg-red-600 hover:bg-red-500 text-white text-xs disabled:opacity-50"
              onclick={() => clickDelete(item.path)}
              disabled={item.status === "deleting"}
              title={item.status === "error" ? "Retry delete" : "Move to trash"}
            >
              {item.status === "error" ? "Retry" : "Delete"}
            </button>
          </div>
        {:else}
          <div class="text-sm text-gray-400 text-center py-8">
            Queue is empty. Click the delete icon on folders or files to target them for deletion.
          </div>
        {/each}
      </div>

      <div class="flex justify-end mt-3">
        <button
          class="px-3 py-1 rounded bg-gray-700 hover:bg-gray-600 text-gray-100"
          onclick={close}
        >
          Close
        </button>
      </div>
    </div>
  </div>
{/if}
