<!-- browser/src/routes/+layout.svelte -->
<script>
  import "../app.css";
  import { onMount } from 'svelte';
  import Login from '../lib/Login.svelte';
  import { State } from '../ts/store.svelte';
  import { clearAll } from '../ts/cache';

  let { children } = $props()

  function parseJwt(token) {
    try {
      const payload = token.split('.')[1] || '';
      let b64 = payload.replace(/-/g, '+').replace(/_/g, '/');
      const pad = b64.length % 4; if (pad) b64 += '='.repeat(4 - pad);
      return JSON.parse(atob(b64));
    } catch { return {}; }
  }

  onMount(() => {
    if (typeof window === 'undefined') return;
    const hash = window.location.hash || '';
    const m = hash.match(/(?:^#|&)token=([^&]+)/);
    if (!m) return;
    const token = decodeURIComponent(m[1]);
    const claims = parseJwt(token);
    State.username = claims?.sub || '';
    State.isAdmin = !!claims?.is_admin;
    State.token = token;
    State.expiresAt = claims?.exp ? claims.exp * 1000 : null;
    try { localStorage.setItem('state', JSON.stringify(State)); } catch {}
    const clean = window.location.pathname + window.location.search;
    window.history.replaceState(null, '', clean);
  });
  // In the desktop build there is no auth — always render the app.
  let authed = $derived(
      __DESKTOP__ || (
        Boolean(State.token)
      && (!State.expiresAt || Date.now() < State.expiresAt)
      )
  )

  function logout() {
    State.logout();
    localStorage.removeItem('state');
    clearAll().catch(() => {});
  }

  function onLogout() {
    logout();
  }

</script>

<div class="flex flex-col h-screen min-h-0 overflow-hidden">
  <div class="flex items-center justify-between p-4 text-xl border-b border-gray-500 text-gray-200 select-none">
    <div class="flex gap-2 items-baseline">
      <div class="self-center"><img src="/icon.svg" width={26} height={26} alt="Dutopia" /></div>
      <div>Dutopia</div>
      <div class="text-xs opacity-60">v{__APP_VERSION__}</div>
    </div>

    <div class="grow"></div>

    <!-- Auth status / logout — hidden in the desktop build. -->
    {#if authed && !__DESKTOP__}
      <div class="flex items-center gap-3 text-sm">
        <span class="opacity-80">{State.username}</span>
        {#if State.isAdmin}
          <span class="px-2 py-1 rounded bg-emerald-600 text-white">Admin</span>
        {/if}
        <button class="px-3 py-1 rounded bg-gray-700 hover:bg-gray-600"
          onclick={onLogout}>
          Logout
        </button>
      </div>
    {/if}
  </div>

  <div class="flex flex-col h-full overflow-hidden p-2 bg-[var(--color)]">
    {#if authed}
      {@render children()}
    {:else}
      <Login />
    {/if}
  </div>
</div>
