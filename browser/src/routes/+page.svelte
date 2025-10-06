<script lang="ts">
  import { onMount } from "svelte";
  import { SvelteMap } from 'svelte/reactivity';
  import { 
    getParent, humanTime, humanCount, humanBytes, getOptimalColors, COLORS,
  } from "../ts/util";
  import { api } from "../ts/api.svelte";
  import { API_URL, State } from "../ts/store.svelte";
  import Svelecte, { addRenderer } from 'svelecte';
  import ColorPicker from 'svelte-awesome-color-picker';
  import PickerButton from '../lib/PickerButton.svelte';
  import PickerWrapper from '../lib/PickerWrapper.svelte';

  //#region state
  let allColors: string[] = []
  let path = $state('/');
  let fullPath = $state('/');
  let folders = $state<FolderItem[]>([]);
  let files = $state<ScannedFile[]>([]);
  let loading = $state(false);
  let initializing = $state(false);
  let progress_current = $state(0);
  let progress_total = $state(0);
  let progress_percent = $state(0);
  let history = $state<string[]>(['/']);
  let histIdx = $state(0);
  type SortKey = "disk" | "size" | "count";
  let sortBy = $state<SortKey>("disk");
  let sortOpen = $state(false);
  let selectedUser = $state("All Users");
  let selectedUserColor = $state('#353599')
  let users = $state<string[]>([]);
  let userColors = $state(new SvelteMap<string, string>());
  let userDropdown = $state<{user:string;color:string}[]>([]);
  let pathInput = $state();
  let isEditing = $state(false);

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
        : _isSelection ? selectedUserColor : item.color;
    const user_css = !State.isAdmin ? "text-gray-400" : ''
    return `<div class="flex gap-2 items-center">
                <div class="border border-gray-500 rounded"
                    style="${icon_base} background: ${icon_bg};">
                </div>
                <div class="${user_css}">${item.user}</div>              
            </div>`    
  }
  addRenderer('color', colorRenderer);

  function createUserDropdown(usernames: string[]) {
    usernames.forEach((uname, index) => {
      if (!userColors.has(uname)) {
        userColors.set(uname, allColors[index % allColors.length]);
      }
    })
    userColors.set('All Users', '')
    userDropdown = Array.from(userColors.entries()).map(([user, color]) => ({user,color}))
  }

  //#endregion

  //#region types
  type Age = {
    count: number;
    disk: number;
    size: number;
    linked: number;
    atime: number;
    mtime: number;
  }
  type RawFolder = {
    path: string;
    users: Record<string, Record<string, Age>>;
  }
  type UserStatsJson = {
    username: string;
    count: number;
    disk: number;
    size: number;
    linked: number;
    atime: number;
    mtime: number;
  }
  type FolderItem = {
    path: string;
    total_count: number;
    total_disk: number;
    total_size: number;
    total_linked: number;
    accessed: number;
    modified: number;
    users: Record<string, UserStatsJson>;
  }
  type ScannedFile = {
    path: string;
    size: number;
    accessed: number;
    modified: number;
    owner: string;
  }
  //#endregion

  //#region age filter
  type AgeFilter = -1 | 0 | 1 | 2;
  let ageFilter = $state<AgeFilter>(-1);
  let ageOpen = $state(false);
  const AGE_LABELS: Record<AgeFilter, string> = {
    '-1': "Any Time",
    0: "Recent",
    1: "Not too old",
    2: "Old files",
  }

  function displayAgeLabel(a: AgeFilter) { 
    return AGE_LABELS[a]; 
  }

  function chooseAge(a: AgeFilter) {
    ageFilter = a;
    ageOpen = false;
    refresh();
  }
  //#endregion

  //#region tooltip
  type Tip = {
    show: boolean;
    x: number;
    y: number;
    username?: string;
    value?: string;
    percent?: number;
  }
  let tip = $state<Tip>({ show: false, x: 0, y: 0 });

  let bubbleEl: HTMLDivElement | null = $state(null);
  const MARGIN = 8;
  const ARROW_GAP = 10;

  function clampToViewport(rawX: number, rawY: number) {
    const ww = window.innerWidth;
    const wh = window.innerHeight;

    const w = bubbleEl?.offsetWidth ?? 200;
    const h = bubbleEl?.offsetHeight ?? 60;

    const halfW = w / 2;

    const minX = MARGIN + halfW;
    const maxX = ww - MARGIN - halfW;

    const minY = MARGIN + h + ARROW_GAP;
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

  //#region folders and files bars

  function transformFolders(raw: RawFolder[], filter: AgeFilter): FolderItem[] {
    const ages: string[] = filter === -1 ? ["0","1","2"] : [String(filter)];

    return (raw ?? [])
      .map((rf) => {
        const usersAgg: Record<string, UserStatsJson> = {};
        let total_count = 0;
        let total_size = 0;
        let total_disk  = 0;
        let total_linked = 0;
        let max_atime   = 0;
        let max_mtime   = 0;

        const userEntries = Object.entries(rf.users ?? {});
        for (const [uname, agesMap] of userEntries) {
          let u_count = 0, u_disk = 0, u_size = 0, u_linked = 0, u_atime = 0, u_mtime = 0;

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
            u_disk  += Number.isFinite(dk) ? dk : 0;
            u_size  += Number.isFinite(sz) ? sz : 0;
            u_linked += Number.isFinite(lk) ? lk : 0;
            if (Number.isFinite(at) && at > u_atime)
              u_atime = at;
            if (Number.isFinite(mt) && mt > u_mtime) 
              u_mtime = mt;
          }

          if (u_count || u_disk) {
            usersAgg[uname] = {
              username: uname,
              count: u_count,
              disk:  u_disk,
              size:  u_size,
              linked: u_linked,
              atime: u_atime,
              mtime: u_mtime,
            };
            total_count += u_count;
            total_disk  += u_disk;
            total_size  += u_size;
            total_linked += u_linked;
            if (u_atime > max_atime) 
              max_atime = u_atime;
            if (u_mtime > max_mtime) 
              max_mtime = u_mtime;
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
      .filter(f => Object.keys(f.users).length > 0);
  }
  
  const toNum = (v: any) => {
    const n = Number(v);
    return Number.isFinite(n) ? n : 0;
  }
  
  // ---------- Sorting (Folders) ----------
  const sortedFolders = $derived.by(() => {
    const key = sortBy;
    const arr = folders ? [...folders] : [];
    return arr.sort((a: any, b: any) => {
      let aVal, bVal;
      switch (key) {
        case "disk":
          aVal = toNum(a?.total_disk);
          bVal = toNum(b?.total_disk);
          break;
        case "size":
          aVal = toNum(a?.total_size);
          bVal = toNum(b?.total_size);
          break;
        case "count":
          aVal = toNum(a?.total_count);
          bVal = toNum(b?.total_count);
          break;
      }
      return bVal - aVal;
    });
  })
  
  let maxMetric = $derived.by(() => {
    const key = sortBy;
    const vals =
      folders?.map((f) => {
        switch (key) {
          case "disk":
            return toNum(f?.total_disk);
          case "size":
            return toNum(f?.total_size);
          case "count":
            return toNum(f?.total_count);
        }
      }) ?? [];
    const max = Math.max(0, ...vals);
    return max > 0 ? max : 1;
  })
  
  const pct = (n: any) => {
    const x = toNum(n);
    const p = (x / maxMetric) * 100;
    const clamped = Math.max(0, Math.min(100, p));
    return Math.round(clamped * 10) / 10;
  }
  
  const displaySortBy = (key: string) => {
    switch (key) {
      case "disk":
        return 'Disk Usage'
      case "size":
        return 'File Size'
      case "count":
        return 'Total Files'
      default:
        return 'Disk Usage'
    }
  }
  
  function clickOutside(
    node: HTMLElement,
    cb: (() => void) | { close: () => void }
  ) {
    let close: (() => void) | undefined =
      typeof cb === "function" ? cb : cb?.close;

    const onPointerDown = (e: PointerEvent) => {
      if (!node.contains(e.target as Node)) close?.();
    };

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape" || e.key === "Esc") close?.();
    };

    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown, true);

    return {
      update(next: typeof cb) {
        close = typeof next === "function" ? next : next?.close;
      },
      destroy() {
        document.removeEventListener("pointerdown", onPointerDown, true);
        document.removeEventListener("keydown", onKeyDown, true);
      },
    };
  }
  
  const metricValue = (file: FolderItem) => {
    switch (sortBy) {
      case "disk":
        return toNum(file?.total_disk);
      case "size":
        return toNum(file?.total_size);
      case "count":
        return toNum(file?.total_count);
    }
  }
  
  function rightValueFolder(folder: FolderItem) {
    switch (sortBy) {
      case "disk":
        return humanBytes(toNum(folder?.total_disk));
      case "size":
        return humanBytes(toNum(folder?.total_size));
      case "count":
        return `Files: ${humanCount(toNum(folder?.total_count))}`;
    }
  }
  
  function bottomValueFolder(folder: FolderItem) {
    switch (sortBy) {
      case "disk":
        return `${humanCount(toNum(folder?.total_count))} Files • Linked: ${humanBytes(toNum(folder?.total_linked))}`;
      case "size":
        return `${humanCount(toNum(folder?.total_count))} Files • Linked: ${humanBytes(toNum(folder?.total_linked))}`;
      case "count":
        return `Disk: ${humanBytes(toNum(folder?.total_disk))} • Linked: ${humanBytes(toNum(folder?.total_linked))}`;
    }
  }
  
  function rightValueFile(f: ScannedFile) {
    switch (sortBy) {
      case "disk":
      case "size":
        return humanBytes(toNum(f.size));
      case "count":
        return "1";
    }
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
  
  const userMetricFor = (ud: UserStatsJson) => {
    switch (sortBy) {
      case "disk":
        return Number(ud.disk);
      case "size":
        return Number(ud.size);
      case "count":
        return Number(ud.count);
    }
  }
  
  function sortedUserEntries(file: FolderItem) {
    return Object.entries(file?.users ?? {}).sort(([, a], [, b]) => userMetricFor(a) - userMetricFor(b));
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
      total_disk  += toNum(f?.total_disk);
      total_size  += toNum(f?.total_size);
      total_linked += toNum(f?.total_linked);
      if (f.accessed > accessed) 
        accessed = f.accessed;
      if (f.modified > modified) 
        modified = f.modified;
      
      const u = f?.users ?? {};
      for (const [uname, data] of Object.entries(u)) {
        const d = data as UserStatsJson;
        const prev = aggUsers[uname] ?? { 
          username: uname, count: 0, disk: 0, size: 0, linked: 0, atime: 0, mtime: 0 
        };
        aggUsers[uname] = {
          username: uname,
          count: prev.count + toNum(d.count),
          disk:  prev.disk  + toNum(d.disk),
          size:  prev.size  + toNum(d.size),
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
      if (file.accessed > accessed) 
        accessed = file.accessed;
      if (file.modified > modified) 
        modified = file.modified;
      
      const owner = file.owner;
      if (owner) {
        const prev = aggUsers[owner] ?? { 
          username: owner, count: 0, disk: 0, size: 0, linked: 0, atime: 0, mtime: 0 
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

    return {
      path: p,
      total_count,
      total_disk,
      total_size,
      total_linked,
      accessed,
      modified,
      users: aggUsers,
    };
  }

  const pathTotals = $derived.by(() => aggregatePathTotals(folders, files, path));
  
  // ---------- Sorting (Files) ----------
  function fileMetricValue(f: ScannedFile) {
    switch (sortBy) {
      case "disk":
      case "size":
        return toNum(f.size);
      case "count":
        return 1;
    }
  }
  
  const sortedfiles = $derived.by(() => {
    const arr = files ? [...files] : [];
    return arr.sort((a, b) => fileMetricValue(b) - fileMetricValue(a));
  });
  
  const maxFileMetric = $derived.by(() => {
    const parentTotal = sortBy === "disk" ? pathTotals.total_disk : 
                       sortBy === "size" ? pathTotals.total_size : 
                       pathTotals.total_count;
    return parentTotal > 0 ? parentTotal : 1;
  });
  
  const filePct = (f: ScannedFile) => Math.round((fileMetricValue(f) / maxFileMetric) * 1000) / 10;
  
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
      const userFilter: string[] = selectedUser === 'All Users' ? [] : [selectedUser];
      const raw: RawFolder[] = await api.getFolders(_p, userFilter, ageFilter);
      console.log("Raw folders:", raw);
      folders = transformFolders(raw, ageFilter);
      files = await api.getFiles(_p, userFilter, ageFilter);
      console.log("Files:", files);
    } finally {
      loading = false;
    }
  })
  //#endregion

  //#region menus

  function pushHistory(p: string) {
    if (history[histIdx] === p) return;
    history = history.slice(0, histIdx + 1);
    history.push(p);
    histIdx = history.length - 1;
  }
  
  function chooseSort(key: SortKey) {
    sortBy = key;
    sortOpen = false;
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
    navigateTo('/');
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
    console.log('selected user:',selectedUser)
    selectedUserColor = userColors.get(selectedUser) ?? '#000000'
    refresh()
  }

  //#endregion

  //#region path

	let copyFeedbackVisible = $state(false);

  function setPath(newPath) {
    const displayedPath = displayPath(newPath);
    fullPath = displayedPath;

    if (!isEditing) {
      path = truncatePathFromStart(displayedPath);
    } else {
      path = displayedPath;
    }
  }
  
  function truncatePathFromStart(inputPath, maxLength = 50) {
    if (!inputPath || inputPath.length <= maxLength) return inputPath;

    const parts = inputPath.split('/');
    let result = parts[parts.length - 1];

    for (let i = parts.length - 2; i >= 0; i--) {
      const potential = parts[i] + '/' + result;
      if (('...' + potential).length > maxLength) break;
      result = potential;
    }

    return '...' + result;
  }
  
  function onPathFocus() {
    isEditing = true;
    if (fullPath) {
      path = fullPath;
    }
  }
  
  function onPathBlur() {
    isEditing = false;
    if (path && !path.startsWith('...')) {
      fullPath = path;
    }
    if (fullPath) {
      path = truncatePathFromStart(fullPath);
    }
  }
  
  function displayPath(p: string): string {
    if (!p) return "/";
    let s = p.replace(/\\/g, "/");
    if (s !== "/") s = s.replace(/\/+$/, "");
    if (!s.startsWith("/")) s = "/" + s;
    return s || "/";
  }
  
  function onPathKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      navigateTo(fullPath || path);
    }
  }

	async function copyText(e) {
    let element = e.currentTarget
    try {
      const textToCopy = element.value !== undefined 
      ? element.value 
      : (element.textContent || element.innerText || '')
      await navigator.clipboard.writeText(textToCopy)
      showCopyFeedback()
    } catch (err) {
      console.error('Copy failed:', err);
    }
	}

	function showCopyFeedback() {
		copyFeedbackVisible = true;
		setTimeout(() => {
			copyFeedbackVisible = false;
		}, 2000);
	}

  //#endregion

  onMount(async () => {
    console.log("api url:", API_URL)
    users = await api.getUsers()
    console.log("Users:", $state.snapshot(users))
    allColors = getOptimalColors(users.length)
    console.log("Colors:", allColors)
    createUserDropdown(users);    
    
    if (State.isAdmin) {
      selectedUser = 'All Users'
    } else {
      selectedUser = State.username
    }
    
    fullPath = path;
    path = truncatePathFromStart(path);
    refresh();
  })
  
</script>

<div class="flex flex-col h-screen min-h-0 gap-2 p-2">
  <div class="flex gap-2 items-center relative select-none">
    <button class="btn" onclick={goHome} title="Go to Root Folder" disabled={histIdx === 0 || fullPath === '/'}>
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

    <!-- Sort dropdown -->
    <div class="relative" use:clickOutside={() => (sortOpen = false)}>
      <button class="btn w-36" onclick={() => (sortOpen = !sortOpen)}>
        <div class="flex items-center gap-2">
          <span class="material-symbols-outlined">sort</span>
          {displaySortBy(sortBy)}
        </div>
      </button>
      {#if sortOpen}
        <div
          class="flex flex-col divide-y divide-gray-500 absolute w-36 rounded border
           border-gray-500 bg-gray-800 shadow-lg z-20 overflow-hidden mt-0.5"
        >
          <button class="w-full text-left px-3 py-2 hover:bg-gray-700 text-nowrap" onclick={() => chooseSort("disk")}>
            By Disk Usage
          </button>
          <button class="w-full text-left px-3 py-2 hover:bg-gray-700 text-nowrap" onclick={() => chooseSort("size")}>
            By File Size
          </button>
          <button class="w-full text-left px-3 py-2 hover:bg-gray-700" onclick={() => chooseSort("count")}>
            By Total Files
          </button>
        </div>
      {/if}
    </div>

    <!-- Age filter dropdown -->
    <div class="relative"  use:clickOutside={() => (ageOpen = false)}>
      <button class="btn w-36" onclick={() => (ageOpen = !ageOpen)}>
        <div class="flex items-center gap-2">
          <span class="material-symbols-outlined">schedule</span>
          {displayAgeLabel(ageFilter)}
        </div>
      </button>
      {#if ageOpen}
        <div
          class="flex flex-col divide-y divide-gray-500 absolute w-48 rounded border
           border-gray-500 bg-gray-800 shadow-lg z-20 overflow-hidden mt-0.5"
        >
          <button class="w-full text-left px-3 py-2 hover:bg-gray-700" onclick={() => chooseAge(-1)}>
            Any Time
          </button>
          <button class="w-full text-left px-3 py-2 hover:bg-gray-700" onclick={() => chooseAge(0)}>
            Recent (2 months)
          </button>
          <button class="w-full text-left px-3 py-2 hover:bg-gray-700" onclick={() => chooseAge(1)}>
            Not too old (2 years)
          </button>
          <button class="w-full text-left px-3 py-2 hover:bg-gray-700" onclick={() => chooseAge(2)}>
            Old files
          </button>
        </div>
      {/if}
    </div>

    <Svelecte
      disabled={!State.isAdmin}
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
      class="z-20 min-w-40 h-10 border rounded
       border-gray-500 bg-gray-800 text-white"
    />
    {#if !selectedUser || selectedUser === 'All Users'}
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
onInput={(e)=>{
          userColors.set(selectedUser, selectedUserColor)
          userDropdown = Array.from(userColors.entries()).map(([user, color]) => ({user,color}))
        }}
      />    
    {/if}
  </div>
  <div class="flex">
    <input 
      bind:this={pathInput}
      bind:value={path} placeholder="Path..." 
      class="w-full truncate text-left cursor-pointer"
      onkeydown={onPathKeydown} 
      onblur={onPathBlur}
      onfocus={onPathFocus}
      onclick={(e)=>copyText(e)}
      autocorrect="off" 
      spellcheck="false"
      autocomplete="off"
      autocapitalize="none"
      disabled={loading} />
  </div>
  
  <!-- Path total header item -->
  <div class="relative px-2 bg-gray-700 border border-gray-500 rounded text-sm p-1">
      <!-- Total bar background -->
      <div class="flex absolute left-0 top-0 bottom-0 z-0" style="width: 100%">
        {#each sortedUserEntries(pathTotals) as [uname, userData] (uname)}
          {@const userMetric = sortBy === "disk" ? userData.disk : sortBy === "size" ? userData.size : userData.count}
          {@const totalMetric =
            sortBy === "disk" ? pathTotals.total_disk : sortBy === "size" ? pathTotals.total_size : pathTotals.total_count}
          {@const userPercent = totalMetric > 0 ? (userMetric / totalMetric) * 100 : 0}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="h-full transition-all duration-300 min-w-[0.5px] hover:opacity-90"
            style="width: {userPercent}%; background-color: {userColors.get(uname)};"
            onmouseenter={(e) => showTip(e, userData, userPercent)}
            onmousemove={moveTip}
            onmouseleave={hideTip}
            aria-label={`${userData.username}: ${rightValueUser(userData)}`}
          ></div>
        {/each}
      </div>
      <!-- Total bar foreground -->
      <div class="relative z-10 pointer-events-none">        
        <div class="flex items-center justify-end">
            {humanCount(pathTotals.total_count)} Files 
            • Changed {humanTime(pathTotals.modified)} 
            • {humanBytes(pathTotals.total_disk)}                            
        </div>
      </div>
    </div>
  
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
    <!-- Skeleton Loader (UI stays interactive) -->
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
      <!-- Folders -->
      {#each sortedFolders as folder}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="relative px-2 py-1 cursor-pointer hover:opacity-95 bg-gray-700 border border-gray-500 rounded-lg overflow-hidden min-h-16"
          onclick={() => navigateTo(folder.path)}
        >
          <!-- Folder bar background -->
          <div class="absolute left-0 top-0 bottom-0 flex z-0" style="width: {pct(metricValue(folder))}%">
            {#each sortedUserEntries(folder) as [uname, userData]}
              {@const userMetric = sortBy === "disk" ? userData.disk : sortBy === "size" ? userData.size : userData.count}
              {@const totalMetric = sortBy === "disk" ? folder.total_disk : sortBy === "size" ? folder.total_size : folder.total_count}
              {@const userPercent = totalMetric > 0 ? (userMetric / totalMetric) * 100 : 0}
              <div
                class="h-full transition-all duration-300 min-w-[0.5px] hover:opacity-90"
                style="width: {userPercent}%; background-color: {userColors.get(uname)};"
                onmouseenter={(e) => showTip(e, userData, userPercent)}
                onmousemove={moveTip}
                onmouseleave={hideTip}
                aria-label={`${userData.username}: ${rightValueUser(userData)}`}
              ></div>
            {/each}
          </div>
          <!-- Folder bar foreground -->
          <div class="relative flex flex-col gap-2 z-10 pointer-events-none">
            <div class="flex items-center justify-between gap-4">
              <div class="w-full overflow-hidden text-ellipsis whitespace-nowrap">
                <div>{folder.path}</div>
              </div>
              <span class="text-nowrap font-bold">{rightValueFolder(folder)}</span>
            </div>
            <div class="flex justify-end">
              <p class="text-sm">
                {bottomValueFolder(folder)}  
                • Updated {humanTime(folder.modified)} 
                {#if humanTime(folder.accessed) > humanTime(folder.modified)}
                • Last file read {humanTime(folder.accessed)} 
                {/if}                      
              </p>
            </div>
          </div>
        </div>
      {/each}

      <!-- Files (after folders) -->
      {#each sortedfiles as f}
        {@const color = userColors.get(f.owner)}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="flex">
          <span class="material-symbols-outlined text-4xl">subdirectory_arrow_right</span>
          <div class="relative flex grow px-2 py-1 bg-gray-700 border border-gray-500 rounded overflow-hidden text-xs">
            <div class="flex flex-col w-full">
              <!-- File bar background -->
              <div class="absolute left-0 top-0 bottom-0 z-0 opacity-60" 
                style="width: {filePct(f)}%; background-color: {color};">
              </div>
              <div class="relative z-10 flex items-center justify-between gap-2">
                <div class="w-full overflow-hidden">
                  <!-- svelte-ignore a11y_click_events_have_key_events -->
                  <span class="cursor-pointer text-ellipsis text-nowrap"
                    onclick={(e)=>copyText(e)}>
                    {f.path}
                  </span>                  
                </div>
                <div class="flex items-center gap-4 text-sm font-semibold text-nowrap">
                  {rightValueFile(f)}
                </div>
              </div>
              <div class="relative z-10 flex justify-between">
                <div class="">{f.owner}</div>
                <div class="">
                  Updated {humanTime(f.modified)} 
                  {#if humanTime(f.accessed) > humanTime(f.modified)}
                  • Read {humanTime(f.accessed)}
                  {/if}
                </div>
              </div>
            </div>
          </div>
        </div>
      {/each}
    </div>
  {/if}
  <div class="grow"></div>
</div>

<!-- Tooltip -->
{#if tip.show}
  <div
    class="fixed z-50 pointer-events-none"
    style="
      left: {tip.x}px;
      top: {tip.y}px;
      transform: translate(-50%, calc(-100% - 10px));
    "
  >
    <div
      bind:this={bubbleEl}
      class="relative rounded-xl border border-white/10 bg-black/90 text-white shadow-xl px-3 py-2"
    >
      <div class="flex items-center justify-center">
        <div class="font-medium text-sm truncate max-w-[180px]">{tip.username}</div>        
      </div>
      <div class="flex gap-2 items-center justify-between text-xs opacity-90">
        <div class="text-nowrap">{tip.value}</div>
        <div class="">{tip.percent}%</div>
      </div>
      <div class="absolute left-1/2 top-full -translate-x-1/2 mt-[-4px]">
        <div class="w-2 h-2 rotate-45 bg-black/90 border border-white/10 border-l-0 border-t-0"></div>
      </div>
    </div>
  </div>
{/if}

<!-- Feedback notification -->
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