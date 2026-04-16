<!-- browser/src/lib/CopyToast.svelte -->
<script lang="ts">
  export let visible: boolean;
</script>

{#if visible}
  <div
    class="fixed top-1 inset-x-0 mx-auto w-max bg-emerald-600 text-white px-4 py-1
      rounded-lg font-medium shadow-lg z-50 transform transition-transform duration-300
      ease-[cubic-bezier(0.68,-0.55,0.265,1.55)]"
    class:translate-x-full={!visible}
    class:translate-x-0={visible}
  >
    Path copied!
  </div>
{/if}
