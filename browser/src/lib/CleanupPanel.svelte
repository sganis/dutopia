<!-- browser/src/lib/CleanupPanel.svelte -->
<!--
  Trash panel (web build only): the server-side trash cart, which also
  holds the items queued for the archive (packed into `<fs>/.archive` as
  .tar.gz by the archive cron, and kept).

  Layout mirrors DeletePanel: a centered modal with a scrollable list of
  items. What's different:
    - No direct-delete button; duapi cannot delete in a cluster deployment.
      Every item is moved to `<fs>/.trash` by the nightly cron once it has
      waited 24h; each row shows when, and Keep (the undo) takes it out of
      the server queue before then.
    - Per-user "Download script" and (admin-only) "Notify user" actions
      remain as extra nudges; they no longer empty the cart.
    - User filter dropdown at the top. Admins see "All users" plus every
      owner currently in the queue. Non-admins only have their own username
      (the dropdown is disabled since there is nothing to pick).
    - Totals on the right of the dropdown reflect the filtered set so the
      admin can see per-user footprint before acting.
    - Download / Notify are enabled only when a specific user is selected
      (the endpoints are per-user). Selecting "All users" is for browsing
      the queue only.
-->
<script lang="ts">
  import { api } from "$api";
  import { State } from "../ts/store.svelte";
  import {
    cleanupQueue,
    loadCleanupQueue,
    loadTrashHistory,
    removeFromCleanupQueue,
    distinctOwners,
    filteredItems,
    trashItemsOf,
    totalsFor,
    downloadScript,
    notifyUser,
  } from "../ts/cleanupQueue.svelte";
  import { humanBytes } from "../ts/util";
  import { trashAfter } from "../ts/trash";

  let {
    onToast,
  }: {
    /** Parent-provided toast hook. Matches the existing showToast pattern in +page.svelte. */
    onToast?: (msg: string) => void;
  } = $props();

  let selectedUser = $state<string | null>(null); // null == All users
  let working = $state(false);

  /** "queued": the cart (Keep possible). "history": what the nightly run did. */
  let tab = $state<"queued" | "history">("queued");
  const owners = $derived(
    tab === "queued"
      ? distinctOwners()
      : [...new Set(cleanupQueue.history.map((h) => h.owner))].sort(),
  );
  const historyItems = $derived(
    cleanupQueue.history.filter((h) => !selectedUser || h.owner === selectedUser),
  );
  const items = $derived(filteredItems(selectedUser));
  const totals = $derived(totalsFor(selectedUser));

  // When the queue changes (e.g. adds from other parts of the app) make
  // sure a previously-selected owner that has been cleared doesn't leave
  // us filtered to an empty set forever.
  $effect(() => {
    if (selectedUser && !owners.includes(selectedUser)) {
      selectedUser = null;
    }
  });

  // Re-read the cart each time the panel opens: the cron may have taken
  // items, or another admin may have queued or kept some.
  $effect(() => {
    if (cleanupQueue.panelOpen) {
      loadCleanupQueue().catch((err) => onToast?.(err?.message ?? "Could not load the trash"));
      loadTrashHistory().catch((err) => onToast?.(err?.message ?? "Could not load the trash history"));
    }
  });

  function close() {
    cleanupQueue.panelOpen = false;
  }

  async function clickKeep(path: string) {
    if (working) return;
    working = true;
    try {
      await removeFromCleanupQueue(path);
      onToast?.("Kept: taken out of the queue");
    } catch (err: any) {
      onToast?.(err?.message ?? "Keep failed");
      await loadCleanupQueue().catch(() => {});
      await loadTrashHistory().catch(() => {});
    } finally {
      working = false;
    }
  }

  function fmtDate(ms: number): string {
    return new Date(ms).toLocaleString(undefined, {
      month: "short", day: "numeric", hour: "2-digit", minute: "2-digit",
    });
  }

  async function clickDownload() {
    if (!selectedUser || working) return;
    working = true;
    try {
      const user = selectedUser;
      await downloadScript(user);
      onToast?.(`Script downloaded for ${user}`);
    } catch (err: any) {
      onToast?.(err?.message ?? "Script download failed");
    } finally {
      working = false;
    }
  }

  async function clickNotify() {
    if (!selectedUser || working) return;
    if (!State.isAdmin) return;
    working = true;
    try {
      const user = selectedUser;
      const to = await notifyUser(user);
      onToast?.(to ? `Email sent to ${to}` : `Email sent to ${user}`);
    } catch (err: any) {
      onToast?.(err?.message ?? "Email send failed");
    } finally {
      working = false;
    }
  }

  const notifyDisabled = $derived(
    tab !== "queued" || !State.isAdmin || !selectedUser || working || !api.smtpConfigured,
  );
  const downloadDisabled = $derived(
    tab !== "queued" || !selectedUser || working || trashItemsOf(selectedUser).length === 0,
  );
</script>

{#if cleanupQueue.panelOpen}
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
      <div class="flex items-center justify-between mb-3 gap-4">
        <div>
          <div class="text-lg font-semibold text-gray-100">Trash &amp; archive</div>
          <div class="text-xs text-gray-400">
            {api.trashUndoHours}h after they are queued, trashed items move to the filesystem's
            .trash and are purged after {api.trashKeepDays} days; archived items are packed into
            its .archive as .tar.gz and kept. Keep undoes it until then.
          </div>
        </div>
        <button
          class="px-2 py-1 rounded bg-gray-700 hover:bg-gray-600 text-gray-100 text-sm"
          onclick={close}
          aria-label="Close"
          title="Close"
        >
          <span class="material-symbols-outlined align-middle text-base">close</span>
        </button>
      </div>

      <div class="flex gap-1 mb-3 text-sm" role="tablist">
        <button
          role="tab"
          aria-selected={tab === "queued"}
          class="px-3 py-1 rounded {tab === 'queued' ? 'bg-gray-600 text-gray-100' : 'text-gray-400 hover:bg-gray-700'}"
          onclick={() => (tab = "queued")}
        >
          Queued ({cleanupQueue.items.length})
        </button>
        <button
          role="tab"
          aria-selected={tab === "history"}
          class="px-3 py-1 rounded {tab === 'history' ? 'bg-gray-600 text-gray-100' : 'text-gray-400 hover:bg-gray-700'}"
          onclick={() => (tab = "history")}
          title="What the nightly run did with items that left the queue"
        >
          History
        </button>
      </div>

      <div class="flex items-center gap-3 mb-3">
        <label class="text-sm text-gray-300" for="cleanup-user-filter">User:</label>
        <select
          id="cleanup-user-filter"
          class="px-2 py-1 rounded border border-gray-500 bg-gray-900 text-gray-100 text-sm min-w-36 disabled:opacity-60"
          bind:value={selectedUser}
          disabled={!State.isAdmin && owners.length <= 1}
        >
          <option value={null}>All users</option>
          {#each owners as owner}
            <option value={owner}>{owner}</option>
          {/each}
        </select>
        <div class="ml-auto text-sm text-gray-300" class:invisible={tab !== "queued"}>
          <span class="text-gray-100 font-medium">{totals.count}</span>
          item{totals.count === 1 ? "" : "s"}
          &nbsp;•&nbsp;
          <span class="text-gray-100 font-medium">{humanBytes(totals.size)}</span>
        </div>
      </div>

      {#if tab === "history"}
      <div class="flex flex-col gap-1 overflow-y-auto flex-1 border border-gray-700 rounded bg-gray-900/40 p-1">
        {#each historyItems as h, i (h.path + h.done_at + i)}
          <div class="p-2 rounded hover:bg-gray-700/40">
            <div class="text-sm text-gray-100 truncate" title={h.path}>{h.path}</div>
            <div class="text-xs text-gray-400">
              {#if h.action === "archive"}
                <span class="text-sky-300">archive</span>&nbsp;•&nbsp;
              {/if}
              {#if h.status === "moved"}
                <span class="text-emerald-400">{h.action === "archive" ? "archived" : "moved"}</span>
                {#if h.dest}&nbsp;to <span class="text-gray-300" title={h.dest}>{h.dest}</span>{/if}
              {:else if h.status === "claimed"}
                <span class="text-sky-400">being moved</span> by the nightly run
              {:else if h.status === "kept"}
                <span class="text-gray-300">kept</span>{#if h.detail}&nbsp;({h.detail}){/if}
              {:else if h.status === "skipped"}
                <span class="text-amber-400">skipped</span>: {h.detail}
              {:else}
                <span class="text-red-400">error</span>: {h.detail}
              {/if}
              &nbsp;•&nbsp; {fmtDate(h.done_at * 1000)}
              &nbsp;•&nbsp; {humanBytes(h.size || 0)}
              &nbsp;•&nbsp; <span class="text-gray-300">{h.owner}</span>
            </div>
          </div>
        {:else}
          <div class="text-sm text-gray-400 text-center py-8">
            Nothing has left the queue yet. The nightly run lists here what it moved, archived or skipped.
          </div>
        {/each}
      </div>
      {:else}
      <div class="flex flex-col gap-1 overflow-y-auto flex-1 border border-gray-700 rounded bg-gray-900/40 p-1">
        {#each items as item (item.path)}
          <div class="flex items-center gap-2 p-2 rounded hover:bg-gray-700/40">
            <div class="flex-1 min-w-0">
              <div class="text-sm text-gray-100 truncate" title={item.path}>{item.path}</div>
              <div class="text-xs text-gray-400">
                {humanBytes(item.size || 0)}
                &nbsp;•&nbsp; <span class="text-gray-300">{item.owner}</span>
                {#if item.actor && item.actor !== item.owner}
                  &nbsp;(queued by {item.actor})
                {/if}
                &nbsp;•&nbsp; <span class:text-sky-300={item.action === "archive"}>{item.action ?? "trash"}</span>
                after {fmtDate(trashAfter(item.queued_at, api.trashUndoHours))}
                {#if item.status === "error" && item.error}
                  &nbsp;•&nbsp; <span class="text-red-400" title={item.error}>{item.error}</span>
                {/if}
              </div>
            </div>

            {#if item.status === "requesting"}
              <span
                class="inline-block w-4 h-4 rounded-full border-2 border-gray-600 border-t-orange-500 animate-spin"
                aria-label="Requesting"
                title="Requesting…"
              ></span>
            {/if}

            <button
              class="px-2 py-0.5 rounded bg-gray-700 hover:bg-gray-600 text-gray-100 text-xs disabled:opacity-50"
              onclick={() => clickKeep(item.path)}
              disabled={working}
              title="Take it out of the queue"
            >
              Keep
            </button>
          </div>
        {:else}
          <div class="text-sm text-gray-400 text-center py-8">
            Nothing queued. Click the trash or archive icon on files or single-owner folders to queue them.
          </div>
        {/each}
      </div>
      {/if}

      <div class="flex justify-end items-center gap-2 mt-3">
        <button
          class="btn inline-flex items-center gap-1"
          onclick={clickDownload}
          disabled={downloadDisabled}
          title={selectedUser ? "Download a Python cleanup script for this user's trash items" : "Pick a user first"}
        >
          <span class="material-symbols-outlined text-base leading-none">download</span>
          <span>Cleanup script</span>
        </button>
        {#if State.isAdmin}
          <button
            class="btn inline-flex items-center gap-1"
            onclick={clickNotify}
            disabled={notifyDisabled}
            title={!api.smtpConfigured
              ? "SMTP not configured on the server"
              : selectedUser
                ? "Email this user the list of their trash items"
                : "Pick a user first"}
          >
            <span class="material-symbols-outlined text-base leading-none">mail</span>
            <span>Notify user</span>
          </button>
        {/if}
        <button class="btn" onclick={close}>Close</button>
      </div>
    </div>
  </div>
{/if}
