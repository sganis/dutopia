<!-- browser/src/lib/Login.svelte -->
<script>
    import { onMount } from 'svelte';
    import { State, API_URL } from '../ts/store.svelte';
    import { fade } from 'svelte/transition';
    import { getOptimalColors } from '../ts/util';

    let username = $state();
    let password = $state();
    let working = $state();
    let error = $state('');
    let mode = $state('password'); // 'password' | 'oidc'
    let oidcLoginUrl = $state('/api/auth/login');
    let url = `${API_URL}/login`;

    onMount(async () => {
        try {
            const r = await fetch(`${API_URL}/auth/mode`);
            if (r.ok) {
                const j = await r.json();
                if (j?.mode === 'oidc') {
                    mode = 'oidc';
                    if (j.login_url) oidcLoginUrl = j.login_url;
                }
            }
        } catch {}
    });

    function onOidcLogin() {
        window.location.href = oidcLoginUrl;
    }

    // minimal base64url → JSON decoder (no signature verification)
    function parseJwt(token) {
        try {
        const payload = token.split('.')[1] || '';
        let b64 = payload.replace(/-/g, '+').replace(/_/g, '/');
        const pad = b64.length % 4; if (pad) b64 += '='.repeat(4 - pad);
        const json = atob(b64);
        return JSON.parse(json);
        } catch {
        return {};
        }
    }

    const onSubmit = async (e) => {
        try {
        working = true;
        error = '';

        const resp = await fetch(url, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'Accept': 'application/json'
            },
            body: JSON.stringify({ username, password }),
        });

        const data = await resp.json().catch(() => ({}));

        if (resp.ok) {
            const token = data.access_token;
            if (!token) {
                error = 'Malformed server response (missing token).';
                return;
            }
            const claims = parseJwt(token)
            const expires_at = claims?.exp ? claims.exp * 1000 : null;

            // update app state
            State.username  = claims?.sub || username;
            State.isAdmin  = !!claims?.is_admin;
            State.token     = token;
            State.expiresAt = expires_at;

            localStorage.setItem('state', JSON.stringify(State));

            // (optional) redirect after login
            // location.href = '/#/';
        } else {
            error = data.error || data.detail || 'Invalid credentials';
        }
        } catch (err) {
            console.error(err);
            error = 'Network error. Please try again.';
        } finally {
            // clear pw from memory/UI
            password = '';
            working = false;
        }
    }

</script>

<div in:fade={{ duration: 500 }}
    class="flex flex-col gap-4 justify-center items-center h-full text-white">
    <div class="flex w-full">
        {#each getOptimalColors(30) as color}
        <div class="w-10 h-2" style="background: {color}"></div>
        {/each}
    </div>
    <div class="flex flex-col gap-4 w-1/2 mt-20 items-center justify-center">
        <div class="flex flex-col gap-2 p-6 border w-1/2 min-w-80
            items-center justify-center
            border-gray-500 bg-gray-800 rounded-lg shadow-lg ">
            {#if mode === 'oidc'}
                <button class="btn w-full" type="button" onclick={onOidcLogin}>
                    Sign in with Keycloak
                </button>
            {:else}
            <form class="space-y-4 w-full" onsubmit={onSubmit}>
                <div>
                    <label class="block text-sm font-medium" for="username">Username</label>
                    <input
                        bind:value={username}
                        class="w-full"
                        id="username"
                        placeholder="Linux user"
                        type="text"
                        required
                        disabled={working}
                    />
                </div>
                <div>
                    <label class="block text-sm font-medium " for="password">Password</label>
                    <input
                        bind:value={password}
                        class="w-full"
                        id="password"
                        placeholder="Linux password"
                        type="password"
                        required
                        disabled={working}
                    />
                </div>
                <div class="flex items-right justify-end">
                    <button
                        class="btn w-full"
                        type="submit"
                        disabled={working}
                    >
                        {#if working}
                            <span class="animate-spin border-2 border-t-transparent rounded-full w-5 h-5 inline-block"></span>
                        {:else}
                            Log in
                        {/if}
                    </button>
                </div>
            </form>
            {/if}
        </div>
    </div>
    {#if error}
        <p class="w-1/2 bg-red-100 text-red-700 p-2 rounded text-sm text-center">{error}</p>
    {/if}
    <div class="grow"></div>
</div>
