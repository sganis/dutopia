<!-- browser/src/lib/PageNav.svelte -->
<script lang="ts">
  import Svelecte from "svelecte";
  import ColorPicker from "svelte-awesome-color-picker";
  import PickerButton from "./PickerButton.svelte";
  import PickerWrapper from "./PickerWrapper.svelte";
  import SortDropdown from "./SortDropdown.svelte";
  import AgeFilter from "./AgeFilter.svelte";

  export let sortBy: "disk" | "size" | "count";
  export let ageFilter: -1 | 0 | 1 | 2;
  export let selectedUser: string;
  export let selectedUserColor: string;
  export let userDropdown: { user: string; color: string }[];
  export let isAdmin: boolean;
  export let histIdx: number;
  export let historyLength: number;
  export let fullPath: string;
  export let path: string;
  export let loading: boolean;
  export let canGoUp: boolean;
  export let goHome: () => void;
  export let goBack: () => void;
  export let goForward: () => void;
  export let goUp: () => void;
  export let onUserChanged: () => void;
  export let onAgeChanged: () => void;
  export let onPathKeydown: (e: KeyboardEvent) => void;
  export let onPathBlur: () => void;
  export let onPathFocus: () => void;
  export let onCopyPath: (e: MouseEvent) => void;
  export let onUserColorInput: () => void;
</script>

<div class="flex gap-2 items-center relative select-none">
  <button class="btn" onclick={goHome} title="Go to Root Folder" disabled={histIdx === 0 || fullPath === "/"}>
    <div class="flex items-center">
      <span class="material-symbols-outlined">home</span>
    </div>
  </button>
  <button class="btn" onclick={goBack} title="Go Back" disabled={histIdx === 0}>
    <div class="flex items-center">
      <span class="material-symbols-outlined">arrow_back</span>
    </div>
  </button>
  <button class="btn" onclick={goForward} title="Go Forward" disabled={histIdx >= historyLength - 1}>
    <div class="flex items-center">
      <span class="material-symbols-outlined">arrow_forward</span>
    </div>
  </button>
  <button class="btn" onclick={goUp} title="Go Up" disabled={!canGoUp}>
    <div class="flex items-center">
      <span class="material-symbols-outlined">arrow_upward</span>
    </div>
  </button>

  <SortDropdown bind:sortBy />
  <AgeFilter bind:ageFilter onchange={onAgeChanged} />

  <Svelecte
    disabled={!isAdmin}
    bind:value={selectedUser}
    options={userDropdown}
    name="user-select"
    valueField="user"
    renderer="color"
    highlightFirstItem={false}
    onChange={onUserChanged}
    closeAfterSelect={true}
    deselectMode="native"
    virtualList={true}
    class="z-20 min-w-40 h-10 border rounded border-gray-500 bg-gray-800 text-white"
  />
  {#if !selectedUser || selectedUser === "All Users"}
    <button class="btn" disabled={true}>
      <div class="flex items-center">
        <span class="material-symbols-outlined">colors</span>
      </div>
    </button>
  {:else}
    <ColorPicker
      bind:hex={selectedUserColor}
      components={{
        input: PickerButton,
        wrapper: PickerWrapper,
      }}
      label="Change User Color"
      onInput={onUserColorInput}
    />
  {/if}
</div>
<div class="flex">
  <input
    bind:value={path}
    placeholder="Path..."
    class="w-full truncate text-left cursor-pointer"
    onkeydown={onPathKeydown}
    onblur={onPathBlur}
    onfocus={onPathFocus}
    onclick={onCopyPath}
    autocorrect="off"
    spellcheck="false"
    autocomplete="off"
    autocapitalize="none"
    disabled={loading}
  />
</div>
