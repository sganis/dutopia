<!-- browser/src/lib/ActionBar.svelte -->
<!--
  Per-row action toolbar.

  Lives at the bottom-left of FolderBar / FileBar. Each prop is an optional
  handler — pass it to render the corresponding icon, omit to hide. To add a
  new action (e.g. delete, share, open in explorer), add a prop here, render
  another <Icon>, and wire it up in the parent. Buttons stop propagation so
  the row's main click handler (folder navigation) is not triggered.
-->
<script lang="ts">
  let {
    onCopy,
    onDelete,
  }: {
    onCopy?: () => void;
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
      <!-- Lucide-style copy: two overlapping rounded rects, crisp at small sizes -->
      <svg
        viewBox="0 0 24 24"
        width="12"
        height="12"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
        <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
      </svg>
    </button>
  {/if}
  {#if onDelete}
    <button
      type="button"
      class="action-btn action-btn-danger"
      title="Delete"
      aria-label="Delete"
      onclick={(e) => handle(e, onDelete)}
    >
      <svg
        viewBox="0 0 24 24"
        width="12"
        height="12"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path d="M3 6h18"></path>
        <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"></path>
        <path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path>
      </svg>
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
  .action-btn:hover {
    opacity: 1;
    background-color: rgba(255, 255, 255, 0.12);
  }
  .action-btn-danger:hover {
    background-color: rgba(239, 68, 68, 0.25);
  }
</style>
