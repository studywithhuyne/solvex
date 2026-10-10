<script lang="ts">
  import { authState } from '../../stores/authStore';
  import { SUPPORTED_ASSET_SYMBOLS } from '../../lib/marketMeta';
  import { coverageStatus } from '../../lib/zkp-verify';

  import { testingConfig } from '../../stores/testingStore';
  const { coldWalletAssets } = testingConfig;

  const AUTO_REFRESH_MS = 10 * 60 * 1000; // 10 minutes

  type SolvencyResult = {
    asset: string;
    snapshot_size: number;
    root_hash: string;
    total_liabilities: string;
    compared_assets: string;
    assets_source: string;
    liabilities_leq_assets: boolean;
    computed_at: string;
  };

  let status = $state<"idle" | "loading" | "unverified" | "error">("idle");
  let errorMsg = $state("");
  let result = $state<SolvencyResult | null>(null);
  let assetFilter = $state("USDT");
  let countdown = $state(AUTO_REFRESH_MS / 1000);

  let intervalId: ReturnType<typeof setInterval> | null = null;
  let countdownId: ReturnType<typeof setInterval> | null = null;

  async function verifySolvency() {
    status = "loading";
    errorMsg = "";

    try {
      const params = new URLSearchParams({ asset: assetFilter });
      const overrideAssets = $coldWalletAssets.trim();
      if (overrideAssets.length > 0) {
        params.set("cold_wallet_assets", overrideAssets);
      }

      const res = await fetch(
        `/api/zkp/solvency?${params.toString()}`,
        { headers: { "x-user-id": ($authState.userId ?? 1).toString() } }
      );
      if (!res.ok) {
        const body = await res.json().catch(() => ({ error: `HTTP ${res.status}` }));
        throw new Error(body.error || `Server returned ${res.status}`);
      }
      const data: SolvencyResult = await res.json();
      result = data;
      status = coverageStatus(data);
    } catch (err: any) {
      status = "error";
      errorMsg = err.message || "Failed to verify solvency";
    }
  }

  function startAutoRefresh() {
    stopAutoRefresh();
    countdown = AUTO_REFRESH_MS / 1000;

    countdownId = setInterval(() => {
      countdown = Math.max(0, countdown - 1);
    }, 1000);

    intervalId = setInterval(() => {
      countdown = AUTO_REFRESH_MS / 1000;
      verifySolvency();
    }, AUTO_REFRESH_MS);
  }

  function stopAutoRefresh() {
    if (intervalId !== null) { clearInterval(intervalId); intervalId = null; }
    if (countdownId !== null) { clearInterval(countdownId); countdownId = null; }
  }

  function formatCountdown(secs: number): string {
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    return `${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
  }

  $effect(() => {
    startAutoRefresh();
    return () => stopAutoRefresh();
  });
</script>

<section class="terminal-panel-strong p-4 sm:p-5 h-full flex flex-col relative overflow-hidden zk-panel">
  <div class="mb-4 flex items-start justify-between gap-3">
    <div>
      <h2 class="text-sm font-semibold tracking-wide text-slate-100 uppercase">
        Reserve Comparison (Demo)
      </h2>
      <p class="mt-0.5 text-[10px] text-slate-500">Simulated amounts; no aggregate proof or reserve evidence</p>
    </div>
    <span class="mono rounded border border-amber-500/25 bg-amber-500/10 px-2 py-0.5 text-[10px] text-amber-200">
      Unverified prototype
    </span>
  </div>

  <div class="flex-1 flex flex-col space-y-4">
    <!-- Controls -->
    <div class="grid gap-2 md:grid-cols-[100px] md:items-center">
      <select
        bind:value={assetFilter}
        class="rounded border border-slate-700/80 bg-slate-900/80 px-2 py-2 text-xs text-slate-200 outline-none focus:border-cyan-500/50 cursor-pointer"
      >
        <option value="USDT">USDT</option>
        {#each SUPPORTED_ASSET_SYMBOLS as symbol}
          <option value={symbol}>{symbol}</option>
        {/each}
      </select>
    </div>

    <!-- Status board -->
    <div class="status-board p-3 rounded-xl border {
      status === 'unverified' ? 'border-amber-500/40 text-amber-300' :
      status === 'error'   ? 'border-orange-500/40 text-orange-300' :
      status === 'loading' ? 'border-cyan-500/40 text-cyan-300' :
      'border-slate-800 text-slate-400'
    }">
      <p class="mono text-[11px] uppercase tracking-[0.15em] font-semibold">
        {#if status === 'idle'}Status: Ready
        {:else if status === 'loading'}Status: Calculating demo comparison...
        {:else if status === 'unverified'}Status: Unverified — simulated comparison
        {:else if status === 'error'}Status: Error
        {/if}
      </p>

      {#if errorMsg}
        <p class="mt-2 text-xs leading-relaxed text-orange-300/95">{errorMsg}</p>
      {/if}
    </div>

    <!-- Result details -->
    {#if result}
      <div class="rounded-xl border border-slate-700/70 bg-slate-950/55 p-3 text-xs text-slate-300/90 space-y-1.5">
        <p class="mono uppercase tracking-[0.14em] text-slate-400 mb-2">Unverified Diagnostic Data</p>
        <p>Asset: <span class="mono text-slate-100">{result.asset}</span></p>
        <p>Root Hash: <span class="mono text-slate-100 break-all text-[10px]">{result.root_hash}</span></p>
        <p>Snapshot Size: <span class="mono text-slate-100">{result.snapshot_size} users</span></p>
        <p>Total Liabilities: <span class="mono text-slate-100">{result.total_liabilities}</span></p>
        <p>Compared Assets: <span class="mono text-slate-100">{result.compared_assets}</span></p>
        <p>Assets Source: <span class="mono text-slate-100">{result.assets_source}</span></p>
        <p class="mt-1 font-semibold text-amber-300">
          Server-reported comparison: {result.liabilities_leq_assets ? 'Liabilities ≤ Declared Assets' : 'Liabilities > Declared Assets'} — unverified
        </p>
        <p class="text-[10px] text-slate-500 mt-1">Computed at: {result.computed_at}</p>
      </div>
    {/if}

    <!-- Auto-refresh countdown -->
    {#if status !== 'idle'}
      <div class="text-center text-[10px] text-slate-500 mono">
        Next refresh in {formatCountdown(countdown)}
      </div>
    {/if}

    <!-- Verify button -->
    <button
      type="button"
      onclick={verifySolvency}
      disabled={status === 'loading'}
      class="w-full h-10 rounded-xl border border-cyan-500/35 bg-cyan-500/18 text-sm font-semibold tracking-wider text-cyan-200 transition hover:bg-cyan-500/26 active:scale-[0.98] disabled:opacity-50 disabled:active:scale-100 uppercase"
    >
      {status === 'loading' ? 'Calculating...' : 'Load Demo Comparison'}
    </button>
  </div>
</section>

<style>
  .zk-panel {
    background:
      radial-gradient(740px 220px at 100% -20%, rgba(251, 191, 36, 0.07), transparent 60%),
      color-mix(in srgb, #020617 84%, transparent);
  }

  .status-board {
    background: rgba(2, 6, 23, 0.6);
  }
</style>
