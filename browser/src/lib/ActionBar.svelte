<!-- browser/src/lib/ActionBar.svelte -->
<!--
  Per-row action toolbar.

  Lives at the bottom-left of FolderBar / FileBar. Each prop is an optional
  handler — pass it to render the corresponding icon, omit to hide. Buttons
  stop propagation so the row's main click handler (folder navigation) is
  not triggered.
-->
<script lang="ts">
  let {
    onCopy,
    onReveal,
    onTerminal,
    onDelete,
    deleteActive = false,
  }: {
    onCopy?: () => void;
    onReveal?: () => void;
    onTerminal?: () => void;
    onDelete?: () => void;
    /** The item is already queued for the trash: the icon stays lit and a
     *  click takes it back out (Keep). */
    deleteActive?: boolean;
  } = $props();

  function handle(e: MouseEvent, fn?: () => void) {
    if (!fn) return;
    e.stopPropagation();
    e.preventDefault();
    fn();
  }
</script>

<div class="flex items-center gap-0.5 pointer-events-auto select-none">
  {#if onCopy}
    <button
      type="button"
      class="action-btn"
      title="Copy path"
      aria-label="Copy path"
      onclick={(e) => handle(e, onCopy)}
    >
      <span class="material-symbols-outlined">content_copy</span>
    </button>
  {/if}
  {#if onReveal}
    <button
      type="button"
      class="action-btn"
      title="Reveal in file manager"
      aria-label="Reveal in file manager"
      onclick={(e) => handle(e, onReveal)}
    >
      <span class="material-symbols-outlined">folder_open</span>
    </button>
  {/if}
  {#if onTerminal}
    <button
      type="button"
      class="action-btn"
      title="Open terminal here"
      aria-label="Open terminal here"
      onclick={(e) => handle(e, onTerminal)}
    >
      <span class="material-symbols-outlined">terminal</span>
    </button>
  {/if}
  {#if onDelete}
    <button
      type="button"
      class="action-btn action-btn-danger"
      class:action-btn-active={deleteActive}
      title={deleteActive ? "Queued for the trash: click to keep it" : "Target for deletion"}
      aria-label={deleteActive ? "Keep (take out of the trash)" : "Target for deletion"}
      aria-pressed={deleteActive}
      onclick={(e) => handle(e, onDelete)}
    >
      <span class="material-symbols-outlined">{deleteActive ? "delete_forever" : "delete"}</span>
      {#if deleteActive}<span class="action-label">in trash · keep</span>{/if}
    </button>
  {/if}
</div>

<style>
  .action-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    border-radius: 3px;
    opacity: 0.5;
    transition: opacity 120ms ease, background-color 120ms ease;
    cursor: pointer;
    color: currentColor;
  }
  .action-btn .material-symbols-outlined {
    font-size: 16px;
    line-height: 1;
  }
  .action-btn:hover {
    opacity: 1;
    background-color: rgba(255, 255, 255, 0.12);
  }
  .action-btn-danger:hover {
    background-color: rgba(239, 68, 68, 0.25);
  }
  /* Readable on any user color: a dark pill with a label. */
  .action-btn-active {
    opacity: 1;
    gap: 3px;
    padding: 1px 6px 1px 4px;
    color: #fff;
    background-color: rgba(0, 0, 0, 0.55);
  }
  .action-btn-active:hover {
    background-color: rgba(0, 0, 0, 0.75);
  }
  .action-label {
    font-size: 11px;
    line-height: 1;
    white-space: nowrap;
  }
</style>
