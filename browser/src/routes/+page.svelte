<!-- browser/src/routes/+page.svelte -->
<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { SvelteMap } from "svelte/reactivity";
  import { getParent, humanTime, humanCount, humanBytes, getOptimalColors, COLORS, escapeHtml } from "../ts/util";
  import { api } from "$api";
  import { API_URL, State } from "../ts/store.svelte";
  import Svelecte, { addRenderer } from "svelecte";
  import ColorPicker from "svelte-awesome-color-picker";
  import PickerButton from "../lib/PickerButton.svelte";
  import PickerWrapper from "../lib/PickerWrapper.svelte";
  import SortDropdown from "../lib/SortDropdown.svelte";
  import AgeFilter from "../lib/AgeFilter.svelte";
  import FolderBar from "../lib/FolderBar.svelte";
  import FileBar from "../lib/FileBar.svelte";
  import PathStats from "../lib/PathStats.svelte";
  import Tooltip from "../lib/Tooltip.svelte";
  import ScanPanel from "../lib/ScanPanel.svelte";
  import StatusBar from "../lib/StatusBar.svelte";
  import DeletePanel from "../lib/DeletePanel.svelte";
  import CleanupPanel from "../lib/CleanupPanel.svelte";
  import { scanStatus } from "../ts/scan.svelte";
  import { deleteQueue, addToQueue } from "../ts/deleteQueue.svelte";
  import { cleanupQueue, addToCleanupQueue } from "../ts/cleanupQueue.svelte";

  //#region types
  type Age = {
    count: number;
    disk: number;
    size: number;
    linked: number;
    atime: number;
    mtime: number;
  };
  type RawFolder = {
    path: string;
    users: Record<string, Record<string, Age>>;
  };
  type UserStatsJson = {
    username: string;
    count: number;
    disk: number;
    size: number;
    linked: number;
    atime: number;
    mtime: number;
  };
  type FolderItem = {
    path: string;
    total_count: number;
    total_disk: number;
    total_size: number;
    total_linked: number;
    accessed: number;
    modified: number;
    users: Record<string, UserStatsJson>;
  };
  type ScannedFile = {
    path: string;
    size: number;
    accessed: number;
    modified: number;
    owner: string;
  };
  type SortKey = "disk" | "size" | "count";
  type AgeFilterType = -1 | 0 | 1 | 2;
  type Tip = {
    show: boolean;
    x: number;
    y: number;
    username?: string;
    value?: string;
    percent?: number;
  };
  //#endregion

  //#region state
  let allColors: string[] = [];
  // Empty string = synthetic trie root (lists drives on Windows, `/` on Unix).
  let path = $state("");
  let fullPath = $state("");
  let folders = $state<FolderItem[]>([]);
  let files = $state<ScannedFile[]>([]);
  let loading = $state(false);
  let initializing = $state(false);
  let progress_current = $state(0);
  let progress_total = $state(0);
  let progress_percent = $state(0);
  let history = $state<string[]>([""]);
  let histIdx = $state(0);
  let sortBy = $state<SortKey>("disk");
  let ageFilter = $state<AgeFilterType>(-1);
  let selectedUser = $state("All Users");
  let selectedUserColor = $state("#353599");
  let users = $state<string[]>([]);
  let userColors = $state(new SvelteMap<string, string>());
  let userDropdown = $state<{ user: string; color: string }[]>([]);
  let pathInput = $state();
  let isEditing = $state(false);
  let copyFeedbackVisible = $state(false);
  let toastMessage = $state("");
  let toastTimer: number | null = null;
  let tip = $state<Tip>({ show: false, x: 0, y: 0 });
  //#endregion

  //#region colors
  function colorRenderer(item, _isSelection, _inputValue) {
    const icon_base = "width:16px;height:16px;border:1px solid gray;border-radius:3px;flex:none;";
    const a = COLORS[0];
    const b = COLORS[1] ?? a;
    const c = COLORS[2] ?? b;
    const d = COLORS[3] ?? c;

    const icon_bg =
      item.user === "All Users"
        ? `linear-gradient(90deg, ${a} 0%, ${b} 33%, ${c} 66%, ${d} 100%)`
        : _isSelection
          ? selectedUserColor
          : item.color;
    const user_css = (!__DESKTOP__ && !State.isAdmin) ? "text-gray-400" : "";
    return `<div class="flex gap-2 items-center">
                <div class="border border-gray-500 rounded"
                    style="${icon_base} background: ${icon_bg};">
                </div>
                <div class="${user_css}">${escapeHtml(item.user)}</div>
            </div>`;
  }
  addRenderer("color", colorRenderer);

  function createUserDropdown(usernames: string[]) {
    usernames.forEach((uname, index) => {
      if (!userColors.has(uname)) {
        userColors.set(uname, allColors[index % allColors.length]);
      }
    });
    userColors.set("All Users", "");
    userDropdown = Array.from(userColors.entries()).map(([user, color]) => ({ user, color }));
  }
  //#endregion

  //#region tooltip
  const MARGIN = 8;
  const ARROW_GAP = 10;
  const TOOLTIP_WIDTH = 200;
  const TOOLTIP_HEIGHT = 60;

  function clampToViewport(rawX: number, rawY: number) {
    const ww = window.innerWidth;
    const wh = window.innerHeight;
    const halfW = TOOLTIP_WIDTH / 2;
    const minX = MARGIN + halfW;
    const maxX = ww - MARGIN - halfW;
    const minY = MARGIN + TOOLTIP_HEIGHT + ARROW_GAP;
    const maxY = wh - MARGIN;
    return {
      x: Math.min(maxX, Math.max(minX, rawX)),
      y: Math.min(maxY, Math.max(minY, rawY)),
    };
  }

  function showTip(e: MouseEvent, userData: UserStatsJson, percent: number) {
    cancelHide();
    const { x, y } = clampToViewport(e.clientX, e.clientY);
    tip = {
      show: true,
      x,
      y,
      username: userData.username,
      value: rightValueUser(userData),
      percent: Math.round(percent * 10) / 10,
    };
  }

  function moveTip(e: MouseEvent) {
    if (!tip.show) return;
    cancelHide();
    const { x, y } = clampToViewport(e.clientX, e.clientY);
    tip = { ...tip, x, y };
  }

  function hideTip() {
    cancelHide();
    tip = { show: false, x: 0, y: 0 };
  }

  const HIDE_DELAY = 1200;
  let hideTimer: number | null = $state(null);

  function scheduleHide(ms = HIDE_DELAY) {
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = window.setTimeout(() => {
      tip = { show: false, x: 0, y: 0 };
      hideTimer = null;
    }, ms);
  }

  function cancelHide() {
    if (hideTimer) {
      clearTimeout(hideTimer);
      hideTimer = null;
    }
  }
  //#endregion

  //#region data transforms
  const toNum = (v: any) => {
    const n = Number(v);
    return Number.isFinite(n) ? n : 0;
  };

  function transformFolders(raw: RawFolder[], filter: AgeFilterType): FolderItem[] {
    const ages: string[] = filter === -1 ? ["0", "1", "2"] : [String(filter)];

    return (raw ?? [])
      .map((rf) => {
        const usersAgg: Record<string, UserStatsJson> = {};
        let total_count = 0;
        let total_size = 0;
        let total_disk = 0;
        let total_linked = 0;
        let max_atime = 0;
        let max_mtime = 0;

        const userEntries = Object.entries(rf.users ?? {});
        for (const [uname, agesMap] of userEntries) {
          let u_count = 0,
            u_disk = 0,
            u_size = 0,
            u_linked = 0,
            u_atime = 0,
            u_mtime = 0;

          for (const a of ages) {
            const s = agesMap?.[a];
            if (!s) continue;

            const c = Number(s.count ?? 0);
            const dk = Number(s.disk ?? 0);
            const sz = Number(s.size ?? 0);
            const lk = Number(s.linked ?? 0);
            const at = Number(s.atime ?? 0);
            const mt = Number(s.mtime ?? 0);

            u_count += Number.isFinite(c) ? c : 0;
            u_disk += Number.isFinite(dk) ? dk : 0;
            u_size += Number.isFinite(sz) ? sz : 0;
            u_linked += Number.isFinite(lk) ? lk : 0;
            if (Number.isFinite(at) && at > u_atime) u_atime = at;
            if (Number.isFinite(mt) && mt > u_mtime) u_mtime = mt;
          }

          if (u_count || u_disk) {
            usersAgg[uname] = {
              username: uname,
              count: u_count,
              disk: u_disk,
              size: u_size,
              linked: u_linked,
              atime: u_atime,
              mtime: u_mtime,
            };
            total_count += u_count;
            total_disk += u_disk;
            total_size += u_size;
            total_linked += u_linked;
            if (u_atime > max_atime) max_atime = u_atime;
            if (u_mtime > max_mtime) max_mtime = u_mtime;
          }
        }

        return {
          path: rf.path,
          total_count,
          total_disk,
          total_size,
          total_linked,
          accessed: max_atime,
          modified: max_mtime,
          users: usersAgg,
        };
      })
      .filter((f) => Object.keys(f.users).length > 0);
  }

  function rightValueUser(userData: UserStatsJson) {
    switch (sortBy) {
      case "disk":
        return humanBytes(toNum(userData?.disk));
      case "size":
        return humanBytes(toNum(userData?.size));
      case "count":
        return humanCount(toNum(userData?.count));
    }
  }

  function aggregatePathTotals(foldersArr: FolderItem[], filesArr: ScannedFile[], p: string): FolderItem {
    let total_count = 0;
    let total_disk = 0;
    let total_size = 0;
    let total_linked = 0;
    let accessed = 0;
    let modified = 0;
    const aggUsers: Record<string, UserStatsJson> = {};

    for (const f of foldersArr ?? []) {
      total_count += toNum(f?.total_count);
      total_disk += toNum(f?.total_disk);
      total_size += toNum(f?.total_size);
      total_linked += toNum(f?.total_linked);
      if (f.accessed > accessed) accessed = f.accessed;
      if (f.modified > modified) modified = f.modified;

      const u = f?.users ?? {};
      for (const [uname, data] of Object.entries(u)) {
        const d = data as UserStatsJson;
        const prev = aggUsers[uname] ?? {
          username: uname,
          count: 0,
          disk: 0,
          size: 0,
          linked: 0,
          atime: 0,
          mtime: 0,
        };
        aggUsers[uname] = {
          username: uname,
          count: prev.count + toNum(d.count),
          disk: prev.disk + toNum(d.disk),
          size: prev.size + toNum(d.size),
          linked: prev.linked + toNum(d.linked),
          atime: Math.max(prev.atime, toNum(d.atime)),
          mtime: Math.max(prev.mtime, toNum(d.mtime)),
        };
      }
    }

    for (const file of filesArr ?? []) {
      total_count += 1;
      total_disk += toNum(file?.size);
      total_size += toNum(file?.size);
      if (file.accessed > accessed) accessed = file.accessed;
      if (file.modified > modified) modified = file.modified;

      const owner = file.owner;
      if (owner) {
        const prev = aggUsers[owner] ?? {
          username: owner,
          count: 0,
          disk: 0,
          size: 0,
          linked: 0,
          atime: 0,
          mtime: 0,
        };
        aggUsers[owner] = {
          username: owner,
          count: prev.count + 1,
          disk: prev.disk + toNum(file.size),
          size: prev.size + toNum(file.size),
          linked: prev.linked,
          atime: Math.max(prev.atime, toNum(file.accessed)),
          mtime: Math.max(prev.mtime, toNum(file.modified)),
        };
      }
    }

    return { path: p, total_count, total_disk, total_size, total_linked, accessed, modified, users: aggUsers };
  }
  //#endregion

  //#region derived state
  const sortedFolders = $derived.by(() => {
    const arr = folders ? [...folders] : [];
    return arr.sort((a, b) => {
      const aVal = sortBy === "disk" ? toNum(a?.total_disk) : sortBy === "size" ? toNum(a?.total_size) : toNum(a?.total_count);
      const bVal = sortBy === "disk" ? toNum(b?.total_disk) : sortBy === "size" ? toNum(b?.total_size) : toNum(b?.total_count);
      return bVal - aVal;
    });
  });

  const maxMetric = $derived.by(() => {
    const vals = folders?.map((f) =>
      sortBy === "disk" ? toNum(f?.total_disk) : sortBy === "size" ? toNum(f?.total_size) : toNum(f?.total_count)
    ) ?? [];
    const max = Math.max(0, ...vals);
    return max > 0 ? max : 1;
  });

  const pct = (folder: FolderItem) => {
    const val = sortBy === "disk" ? toNum(folder?.total_disk) : sortBy === "size" ? toNum(folder?.total_size) : toNum(folder?.total_count);
    const p = (val / maxMetric) * 100;
    return Math.round(Math.max(0, Math.min(100, p)) * 10) / 10;
  };

  const pathTotals = $derived.by(() => aggregatePathTotals(folders, files, path));

  const sortedfiles = $derived.by(() => {
    const arr = files ? [...files] : [];
    return arr.sort((a, b) => {
      const aVal = sortBy === "count" ? 1 : toNum(a.size);
      const bVal = sortBy === "count" ? 1 : toNum(b.size);
      return bVal - aVal;
    });
  });

  const maxFileMetric = $derived.by(() => {
    const parentTotal =
      sortBy === "disk" ? pathTotals.total_disk : sortBy === "size" ? pathTotals.total_size : pathTotals.total_count;
    return parentTotal > 0 ? parentTotal : 1;
  });

  const filePct = (f: ScannedFile) => {
    const val = sortBy === "count" ? 1 : toNum(f.size);
    return Math.round((val / maxFileMetric) * 1000) / 10;
  };
  //#endregion

  //#region fetch data
  function createDoItAgain<T extends any[]>(fn: (...args: T) => Promise<void>) {
    let running = false;
    let nextArgs: T | null = null;
    return async (...args: T) => {
      nextArgs = args;
      if (running) return;
      running = true;
      try {
        while (nextArgs) {
          const argsNow = nextArgs;
          nextArgs = null;
          await fn(...(argsNow as T));
        }
      } finally {
        running = false;
      }
    };
  }

  const fetchFolders = createDoItAgain(async (_p: string) => {
    loading = true;
    try {
      const userFilter: string[] = selectedUser === "All Users" ? [] : [selectedUser];
      const raw: RawFolder[] = await api.getFolders(_p, userFilter, ageFilter);
      folders = transformFolders(raw, ageFilter);
      // /api/files is a live directory walk and rejects the roots ("", "/").
      files = _p === "" || _p === "/" ? [] : await api.getFiles(_p, userFilter, ageFilter);
    } finally {
      loading = false;
    }
  });
  //#endregion

  //#region navigation
  function pushHistory(p: string) {
    if (history[histIdx] === p) return;
    history = history.slice(0, histIdx + 1);
    history.push(p);
    histIdx = history.length - 1;
  }

  function navigateTo(p) {
    setPath(p);
    pushHistory(fullPath || path);
    fetchFolders(fullPath || path);
  }

  function refresh() {
    fetchFolders(fullPath || path);
  }

  function goHome() {
    // Empty path = synthetic trie root; lists top-level entries (drives on
    // Windows, `/` on Unix).
    navigateTo("");
  }

  function goUp() {
    const parent = getParent(fullPath || path);
    navigateTo(parent);
  }

  function goBack() {
    if (histIdx > 0) {
      histIdx -= 1;
      setPath(history[histIdx]);
      fetchFolders(history[histIdx]);
    }
  }

  function goForward() {
    if (histIdx < history.length - 1) {
      histIdx += 1;
      setPath(history[histIdx]);
      fetchFolders(history[histIdx]);
    }
  }

  function onUserChanged() {
    // "All Users" stores an empty string in userColors on purpose; ?? doesn't
    // treat "" as nullish, so use || to fall through to a valid hex.
    selectedUserColor = userColors.get(selectedUser) || "#353599";
    refresh();
  }
  //#endregion

  //#region path
  function setPath(newPath: string) {
    const displayedPath = displayPath(newPath);
    fullPath = displayedPath;
    path = isEditing ? displayedPath : truncatePathFromStart(displayedPath);
  }

  function truncatePathFromStart(inputPath: string, maxLength = 50): string {
    if (!inputPath || inputPath.length <= maxLength) return inputPath;
    const sep = inputPath.includes("\\") ? "\\" : "/";
    const parts = inputPath.split(sep);
    let result = parts[parts.length - 1];
    for (let i = parts.length - 2; i >= 0; i--) {
      const potential = parts[i] + sep + result;
      if (("..." + potential).length > maxLength) break;
      result = potential;
    }
    return "..." + result;
  }

  function onPathFocus() {
    isEditing = true;
    if (fullPath) path = fullPath;
  }

  function onPathBlur() {
    isEditing = false;
    if (path && !path.startsWith("...")) fullPath = displayPath(path);
    if (fullPath) path = truncatePathFromStart(fullPath);
  }

  /**
   * Minimal normalization: trim trailing separators, preserve OS-native form.
   *   `C:\Users\San\` -> `C:\Users\San`
   *   `C:\`           -> `C:\`     (drive root keeps the backslash — stripping
   *                                  it would turn the path into a drive-
   *                                  relative reference like `F:foo.txt`)
   *   `/var/log/`     -> `/var/log`
   *   `/`             -> `/`
   *   empty           -> ``       (synthetic trie root)
   */
  function displayPath(p: string): string {
    if (!p) return "";
    const s = p.trim();
    if (s === "/" || s === "\\") return s;
    if (/^[A-Za-z]:\\$/.test(s)) return s;
    if (s.includes("\\")) return s.replace(/\\+$/, "") || "\\";
    return s.replace(/\/+$/, "") || "/";
  }

  function onPathKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") navigateTo(fullPath || path);
  }

  async function copyText(e) {
    let element = e.currentTarget;
    try {
      const textToCopy = element.value !== undefined ? element.value : element.textContent || element.innerText || "";
      await navigator.clipboard.writeText(textToCopy);
      showCopyFeedback();
    } catch (err) {
      console.error("Copy failed:", err);
    }
  }

  async function copyPath(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      showCopyFeedback();
    } catch (err) {
      console.error("Copy failed:", err);
    }
  }

  function showCopyFeedback() {
    copyFeedbackVisible = true;
    setTimeout(() => {
      copyFeedbackVisible = false;
    }, 2000);
  }
  //#endregion

  //#region desktop-only actions
  async function onScanComplete(scannedPaths: string[]) {
    // Scan replaced the DB. Refresh users (list may have changed) and jump
    // into the scanned path so the user sees the fresh results immediately —
    // landing on the synthetic root would show only platform roots (e.g.
    // `C:\`), which visually looks identical to the pre-scan state even
    // though the stats underneath are fresh.
    users = await api.getUsers();
    allColors = getOptimalColors(users.length);
    userColors = new SvelteMap<string, string>();
    createUserDropdown(users);
    selectedUser = "All Users";
    // If multiple paths were scanned, navigate to the nearest common ancestor
    // (the synthetic root) so all are visible in the listing. For a single
    // path, drill straight in.
    const target = scannedPaths.length === 1 ? scannedPaths[0] : "";
    // Reset history so Back doesn't return to stale pre-scan state.
    history = [target];
    histIdx = 0;
    setPath(target);
    fetchFolders(target);
  }

  async function revealPath(p: string) {
    try { await api.revealPath(p); } catch (err) { console.error("Reveal failed:", err); }
  }
  async function openTerminal(p: string) {
    try { await api.openTerminal(p); } catch (err) { console.error("Open terminal failed:", err); }
  }
  /** Trash-icon click on a row — stage the path for deletion/cleanup.
   *  Desktop builds end with an actual trash call via DeletePanel; web
   *  builds end with a script download or email notify via CleanupPanel.
   *  The two queues are separate so their localStorage never collides. */
  function targetForDeletion(p: string, size: number, owner: string) {
    if (__DESKTOP__) {
      addToQueue(p, size);
      showToast(`Targeted for deletion (${deleteQueue.items.length})`);
    } else {
      addToCleanupQueue(p, size, owner);
      showToast(`Targeted for cleanup (${cleanupQueue.items.length})`);
    }
  }

  function showToast(msg: string) {
    toastMessage = msg;
    if (toastTimer !== null) clearTimeout(toastTimer);
    toastTimer = window.setTimeout(() => {
      toastMessage = "";
      toastTimer = null;
    }, 2000);
  }
  //#endregion

  onMount(async () => {
    users = await api.getUsers();
    allColors = getOptimalColors(users.length);
    createUserDropdown(users);

    if (__DESKTOP__ || State.isAdmin) {
      selectedUser = "All Users";
    } else {
      selectedUser = State.username;
    }

    // Web build: probe /api/health once so the cleanup panel can disable
    // the Notify button when SMTP isn't configured on the server.
    if (!__DESKTOP__) {
      api.probeHealth();
    }

    fullPath = path;
    path = truncatePathFromStart(path);
    refresh();
  });

  onDestroy(() => {
    if (toastTimer !== null) {
      clearTimeout(toastTimer);
      toastTimer = null;
    }
  });
</script>

<div class="flex flex-col h-screen min-h-0 gap-2 p-2">
  <div class="flex gap-2 items-center relative select-none">
    <button class="btn" onclick={goHome} title="Go to Root Folder" disabled={fullPath === ""}>
      <div class="flex items-center">
        <span class="material-symbols-outlined">home</span>
      </div>
    </button>
    <button class="btn" onclick={goBack} title="Go Back" disabled={histIdx === 0}>
      <div class="flex items-center">
        <span class="material-symbols-outlined">arrow_back</span>
      </div>
    </button>
    <button class="btn" onclick={goForward} title="Go Forward" disabled={histIdx >= history.length - 1}>
      <div class="flex items-center">
        <span class="material-symbols-outlined">arrow_forward</span>
      </div>
    </button>
    <button class="btn" onclick={goUp} title="Go Up" disabled={getParent(path) === path}>
      <div class="flex items-center">
        <span class="material-symbols-outlined">arrow_upward</span>
      </div>
    </button>

    <SortDropdown bind:sortBy />
    <AgeFilter bind:ageFilter onchange={refresh} />

    {#if __DESKTOP__}
      <ScanPanel onComplete={onScanComplete} />
    {/if}

    <Svelecte
      disabled={!__DESKTOP__ && !State.isAdmin}
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
      class="z-20 grow min-w-40 h-10 border rounded border-gray-500 bg-gray-800 text-white"
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
        onInput={(e) => {
          userColors.set(selectedUser, selectedUserColor);
          userDropdown = Array.from(userColors.entries()).map(([user, color]) => ({ user, color }));
        }}
      />
    {/if}

    {#if __DESKTOP__}
      <button
        class="btn relative"
        onclick={() => (deleteQueue.panelOpen = true)}
        title="Review delete queue"
        disabled={deleteQueue.items.length === 0}
      >
        <div class="flex items-center gap-1">
          <span class="material-symbols-outlined">delete_sweep</span>
          {#if deleteQueue.items.length > 0}
            <span
              class="absolute -top-1 -right-1 min-w-[1.1rem] h-[1.1rem] px-1 rounded-full bg-red-600 text-white text-[0.65rem] leading-[1.1rem] text-center font-semibold"
            >
              {deleteQueue.items.length}
            </span>
          {/if}
        </div>
      </button>
    {:else}
      <button
        class="btn relative"
        onclick={() => (cleanupQueue.panelOpen = true)}
        title="Review cleanup queue"
        disabled={cleanupQueue.items.length === 0}
      >
        <div class="flex items-center gap-1">
          <span class="material-symbols-outlined">delete_sweep</span>
          {#if cleanupQueue.items.length > 0}
            <span
              class="absolute -top-1 -right-1 min-w-[1.1rem] h-[1.1rem] px-1 rounded-full bg-red-600 text-white text-[0.65rem] leading-[1.1rem] text-center font-semibold"
            >
              {cleanupQueue.items.length}
            </span>
          {/if}
        </div>
      </button>
    {/if}
  </div>
  <div class="flex">
    <input
      bind:this={pathInput}
      bind:value={path}
      placeholder="Path..."
      class="w-full truncate text-left cursor-pointer"
      onkeydown={onPathKeydown}
      onblur={onPathBlur}
      onfocus={onPathFocus}
      onclick={(e) => copyText(e)}
      autocorrect="off"
      spellcheck="false"
      autocomplete="off"
      autocapitalize="none"
      disabled={loading}
    />
  </div>

  <PathStats {pathTotals} {sortBy} {userColors} onUserHover={showTip} onUserMove={moveTip} onUserLeave={hideTip} />

  {#if initializing}
    <div class="flex flex-col w-full h-full items-center justify-between font-mono">
      <div class="w-full bg-gray-700 rounded-full h-1">
        <div class="bg-orange-500 h-1 rounded-full transition-all duration-300" style="width: {progress_percent}%"></div>
      </div>
      <div class="flex flex-col justify-center grow items-center w-64">
        <div class="flex w-full justify-between">
          <div>Progress:</div>
          <div>{progress_percent}%</div>
        </div>
        <div class="flex w-full justify-between">
          <div>Loaded folders:</div>
          <div>{progress_current}</div>
        </div>
        <div class="flex w-full justify-between">
          <div>Total:</div>
          <div>{progress_total}</div>
        </div>
      </div>
    </div>
  {:else if loading}
    <div class="flex flex-col gap-2 overflow-y-auto">
      {#each Array(6) as _, i}
        <div class="relative p-3 bg-gray-800 border border-gray-500 rounded-lg animate-pulse min-h-16 h-16">
          <div class="flex items-center justify-between gap-4">
            <div class="h-4 bg-gray-700 rounded w-3/4"></div>
            <div class="h-3 bg-gray-700 rounded w-12"></div>
          </div>
          <div class="flex gap-2 mt-2">
            <div class="h-3 bg-gray-700 rounded w-16"></div>
            <div class="h-3 bg-gray-700 rounded w-20"></div>
            <div class="h-3 bg-gray-700 rounded w-24"></div>
          </div>
        </div>
      {/each}
    </div>
  {:else}
    <div class="flex flex-col gap-2 overflow-y-auto transition-opacity duration-200 p-4">
      {#if __DESKTOP__ && sortedFolders.length === 0 && sortedfiles.length === 0 && path === ""}
        <div class="flex flex-col items-center justify-center text-center text-gray-300 py-16 gap-3">
          <span class="material-symbols-outlined text-6xl opacity-50">travel_explore</span>
          <div class="text-lg">No data yet</div>
          <div class="text-sm opacity-70 max-w-md">
            Click <span class="font-semibold">Scan</span> above to index your filesystem.
            A scan runs duscan, dusum, and dudb in the background.
          </div>
        </div>
      {/if}
      {#each sortedFolders as folder}
        <FolderBar
          {folder}
          {sortBy}
          widthPercent={pct(folder)}
          {userColors}
          onclick={() => navigateTo(folder.path)}
          onCopyPath={copyPath}
          onReveal={__DESKTOP__ ? revealPath : undefined}
          onTerminal={__DESKTOP__ ? openTerminal : undefined}
          onDelete={targetForDeletion}
          onUserHover={showTip}
          onUserMove={moveTip}
          onUserLeave={hideTip}
        />
      {/each}

      {#each sortedfiles as file}
        <FileBar
          {file}
          {sortBy}
          widthPercent={filePct(file)}
          {userColors}
          onCopyPath={copyPath}
          onDelete={targetForDeletion}
        />
      {/each}
    </div>
  {/if}
  <div class="grow"></div>
</div>

<Tooltip {tip} />

{#if __DESKTOP__}
  <StatusBar />
  <DeletePanel onChange={refresh} />
{:else}
  <CleanupPanel onToast={showToast} />
{/if}

{#if api.error}
  <div
    class="fixed bottom-4 inset-x-0 mx-auto w-max max-w-lg bg-red-600 text-white px-5 py-2
      rounded-lg font-medium shadow-lg z-50 flex items-center gap-3"
  >
    <span class="material-symbols-outlined text-lg">error</span>
    <span>{api.error}</span>
    <button class="ml-2 opacity-70 hover:opacity-100" onclick={() => { api.error = ''; }}>
      <span class="material-symbols-outlined text-sm">close</span>
    </button>
  </div>
{/if}

{#if copyFeedbackVisible}
  <div
    class="fixed top-1 inset-x-0 mx-auto w-max bg-emerald-600 text-white px-4 py-1
      rounded-lg font-medium shadow-lg z-50 transform transition-transform duration-300
      ease-[cubic-bezier(0.68,-0.55,0.265,1.55)]"
    class:translate-x-full={!copyFeedbackVisible}
    class:translate-x-0={copyFeedbackVisible}
  >
    Path copied!
  </div>
{/if}

{#if toastMessage}
  <div
    class="fixed top-1 inset-x-0 mx-auto w-max bg-gray-800 text-gray-100 border border-gray-600 px-4 py-1
      rounded-lg font-medium shadow-lg z-50"
  >
    {toastMessage}
  </div>
{/if}
