<!-- browser/src/lib/ConfirmDialog.svelte -->
<!--
  Minimal modal confirm dialog. Parent controls visibility via `open` and
  reacts to `onConfirm` / `onCancel`. Used for destructive actions like
  "move to trash".
-->
<script lang="ts">
  let {
    open = false,
    title = "Confirm",
    message = "",
    confirmLabel = "Confirm",
    cancelLabel = "Cancel",
    danger = false,
    onConfirm,
    onCancel,
  }: {
    open?: boolean;
    title?: string;
    message?: string;
    confirmLabel?: string;
    cancelLabel?: string;
    danger?: boolean;
    onConfirm: () => void;
    onCancel: () => void;
  } = $props();

  function keydown(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === "Escape") onCancel();
    if (e.key === "Enter") onConfirm();
  }
</script>

<svelte:window onkeydown={keydown} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
    onclick={onCancel}
  >
    <div
      class="bg-gray-800 border border-gray-500 rounded-lg shadow-lg p-5 min-w-80 max-w-md"
      onclick={(e) => e.stopPropagation()}
    >
      <div class="text-lg font-semibold mb-2 text-gray-100">{title}</div>
      <div class="text-sm text-gray-200 mb-4 whitespace-pre-line">{message}</div>
      <div class="flex justify-end gap-2">
        <button
          class="px-3 py-1 rounded bg-gray-700 hover:bg-gray-600 text-gray-100"
          onclick={onCancel}
        >
          {cancelLabel}
        </button>
        <button
          class="px-3 py-1 rounded text-white {danger ? 'bg-red-600 hover:bg-red-500' : 'bg-emerald-600 hover:bg-emerald-500'}"
          onclick={onConfirm}
        >
          {confirmLabel}
        </button>
      </div>
    </div>
  </div>
{/if}
