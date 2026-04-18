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
  }: {
    onCopy?: () => void;
    onReveal?: () => void;
    onTerminal?: () => void;
    onDelete?: () => void;
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
      title="Target for deletion"
      aria-label="Target for deletion"
      onclick={(e) => handle(e, onDelete)}
    >
      <span class="material-symbols-outlined">delete</span>
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
</style>
