<script lang="ts">
  import { router } from "../../stores/routerStore";
  import { connectionState } from "../../stores/appStore";
  import { authState, logout, updateDisplayName } from "../../stores/authStore";
  import type { Route } from "../../stores/routerStore";

  const PUBLIC_LINKS: { route: Route; label: string }[] = [
    { route: "/", label: "Markets" },
    { route: "/login", label: "Login" },
  ];

  const AUTH_LINKS: { route: Route; label: string }[] = [
    { route: "/", label: "Markets" },
    { route: "/trade", label: "Trade" },
    { route: "/zk-verify", label: "ZK Verify" },
  ];

  async function handleEditDisplayName() {
    const current = $authState.displayName ?? "";
    const next = window.prompt("Enter new display name", current);
    if (!next) return;

    try {
      await updateDisplayName(next);
    } catch (error) {
      const message = error instanceof Error ? error.message : "Failed to update display name";
      window.alert(message);
    }
  }

  function handleLogout() {
    logout();
    router.navigate("/login");
  }
</script>

<nav class="terminal-panel relative z-90 overflow-visible px-4 py-3 sm:px-6">
  <div class="flex items-center justify-between gap-4">
    <!-- Brand -->
    <div class="flex items-center gap-3 shrink-0">
      <p class="mono text-xs tracking-[0.18em] text-sky-300/80">SOLVEX</p>
    </div>

    <!-- Navigation Links -->
    {#if $authState.userId}
      <div class="flex items-center gap-1 rounded-lg bg-slate-900/60 p-1 border border-slate-800/60">
        {#each AUTH_LINKS as link}
          <a
            href="#{link.route}"
            class="px-3 py-1.5 rounded-md text-xs font-medium tracking-wide transition-all
              {$router === link.route
                ? 'bg-sky-500/15 text-sky-300 border border-sky-400/30'
                : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/50 border border-transparent'}"
          >
            {link.label}
          </a>
        {/each}
      </div>
    {:else}
      <div class="flex items-center gap-1 rounded-lg bg-slate-900/60 p-1 border border-slate-800/60">
        {#each PUBLIC_LINKS as link}
          <a
            href="#{link.route}"
            class="px-3 py-1.5 rounded-md text-xs font-medium tracking-wide transition-all
              {$router === link.route
                ? 'bg-sky-500/15 text-sky-300 border border-sky-400/30'
                : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/50 border border-transparent'}"
          >
            {link.label}
          </a>
        {/each}
      </div>
    {/if}

    <!-- User + Status -->
    <div class="flex items-center gap-3 shrink-0">
      {#if $authState.userId}
        <div class="relative z-95 group/user-menu">
          <button
            type="button"
            class="mono rounded-lg border border-slate-700/80 bg-slate-900/80 px-2.5 py-1.5 text-xs text-slate-100 transition hover:border-slate-500"
          >
            {$authState.displayName}
          </button>

          <div
            class="invisible absolute right-0 top-[calc(100%+0.45rem)] z-120 w-56 rounded-lg border border-slate-700/80 bg-slate-950/95 p-1.5 opacity-0 shadow-xl shadow-black/40 transition-all duration-150 group-hover/user-menu:visible group-hover/user-menu:opacity-100 group-focus-within/user-menu:visible group-focus-within/user-menu:opacity-100"
          >
            <div class="mb-1.5 rounded-md border border-slate-800 bg-slate-900/70 px-2 py-1.5">
              <p class="text-[10px] uppercase tracking-wider text-slate-500">Signed in</p>
              <p class="mono text-xs text-slate-200">{$authState.displayName}</p>
              <p class="mono text-[10px] text-cyan-300">ID: {$authState.userId}</p>
            </div>

            <a href="#/asset" class="block rounded-md px-2 py-1.5 text-xs text-slate-300 transition hover:bg-slate-800/80 hover:text-sky-300">Asset</a>
            <a href="#/trade-history" class="block rounded-md px-2 py-1.5 text-xs text-slate-300 transition hover:bg-slate-800/80 hover:text-sky-300">Trade History</a>

            <button
              type="button"
              class="mt-1 block w-full rounded-md px-2 py-1.5 text-left text-xs text-amber-300 transition hover:bg-slate-800/80"
              onclick={handleEditDisplayName}
            >
              Edit Display Name
            </button>

            <button
              type="button"
              class="block w-full rounded-md px-2 py-1.5 text-left text-xs text-rose-300 transition hover:bg-slate-800/80"
              onclick={handleLogout}
            >
              Logout
            </button>
          </div>
        </div>
      {/if}

      <div class="flex items-center gap-1.5" title="WebSocket: {$connectionState.ws}">
        <span
          class="inline-block h-2 w-2 rounded-full {$connectionState.ws === 'connected' ? 'bg-emerald-400 shadow-[0_0_6px_rgba(16,185,129,0.6)]' : 'bg-rose-400 shadow-[0_0_6px_rgba(244,63,94,0.6)]'}"
        ></span>
        <span class="mono text-[10px] uppercase tracking-wider {$connectionState.ws === 'connected' ? 'text-emerald-400' : 'text-rose-400'}">
          {$connectionState.ws}
        </span>
      </div>
    </div>
  </div>
</nav>
