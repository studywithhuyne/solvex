<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { router } from "../../stores/routerStore";
  import { selectedMarket } from "../../stores/marketStore";
  import { SUPPORTED_MARKET_ASSETS } from "../../lib/marketMeta";
  import { fetchLiveTickers } from "../../lib/api/client";

  type MarketRow = {
    symbol: string;
    iconUrl: string;
    price: string;
    volume24h: string;
    changePct24h: string;
    isPositive: boolean;
  };

  const MARKETS = SUPPORTED_MARKET_ASSETS;
  const REFRESH_MS = 12_000;

  let isLoading = $state(true);
  let rows = $state<MarketRow[]>([]);
  let updatedAt = $state<string>("");
  let refreshTimer: ReturnType<typeof setInterval> | null = null;

  function formatPrice(value?: string | null): string {
    if (!value) return "--";
    const n = Number(value);
    return Number.isFinite(n) ? n.toLocaleString(undefined, { maximumFractionDigits: 4 }) : value;
  }

  function formatVolume(value?: string | null): string {
    if (!value) return "--";
    const n = Number(value);
    return Number.isFinite(n) ? n.toLocaleString(undefined, { maximumFractionDigits: 2 }) : value;
  }

  async function loadMarkets() {
    try {
      const data = await fetchLiveTickers(MARKETS.map((m) => `${m.symbol}USDT`));
      const bySymbol = new Map<string, any>(data.map((item: any) => [item.symbol, item]));

      const results: MarketRow[] = MARKETS.map((market) => {
        const key = `${market.symbol}USDT`;
        const ticker = bySymbol.get(key);
        const pct = Number(ticker?.price_change_percent_24h ?? 0);
        const sign = pct > 0 ? "+" : "";

        return {
          symbol: market.pair,
          iconUrl: market.iconUrl,
          price: formatPrice(ticker?.last_price ?? null),
          volume24h: formatVolume(ticker?.quote_volume_24h ?? null),
          changePct24h: Number.isFinite(pct) ? `${sign}${pct.toFixed(2)}%` : "--",
          isPositive: pct >= 0,
        };
      });

      rows = results;
      updatedAt = new Date().toLocaleTimeString();
    } catch {
      rows = MARKETS.map((market) => ({
        symbol: market.pair,
        iconUrl: market.iconUrl,
        price: "--",
        volume24h: "--",
        changePct24h: "--",
        isPositive: true,
      }));
    } finally {
      isLoading = false;
    }
  }

  function openTerminal(symbol: string) {
    // Keep selected pair for future terminal support while routing to trade now.
    localStorage.setItem("preferred_trade_symbol", symbol);
    selectedMarket.set(symbol);
    router.navigate("/trade");
  }

  onMount(() => {
    void loadMarkets();
    refreshTimer = setInterval(() => {
      void loadMarkets();
    }, REFRESH_MS);
  });

  onDestroy(() => {
    if (refreshTimer) {
      clearInterval(refreshTimer);
    }
  });
</script>

<section class="space-y-8 md:space-y-10 pb-12">
  <!-- Hero Area -->
  <header class="landing-hero relative overflow-hidden rounded-3xl border border-slate-800 bg-slate-900/50 px-6 py-16 md:px-12 md:py-24 text-center shadow-2xl">
    <div class="absolute inset-0 bg-[radial-gradient(ellipse_at_top,var(--tw-gradient-stops))] from-slate-800/40 via-slate-900/5 to-transparent pointer-events-none"></div>
    
    <div class="relative z-10 mx-auto max-w-4xl">
      <div class="mb-8 inline-flex items-center gap-2 rounded-full border border-cyan-500/30 bg-cyan-500/10 px-4 py-1.5 text-xs font-semibold uppercase tracking-widest text-cyan-300">
        <span class="relative flex h-2 w-2">
          <span class="absolute inline-flex h-full w-full animate-ping rounded-full bg-cyan-400 opacity-75"></span>
          <span class="relative inline-flex h-2 w-2 rounded-full bg-cyan-500"></span>
        </span>
        ZKP-Powered Matching Engine
      </div>

      <h1 class="mb-6 text-4xl font-extrabold tracking-tight text-white md:text-5xl lg:text-6xl">
        Trade at the Speed of Light <br class="hidden md:block" />
        <span class="bg-linear-to-r from-cyan-400 via-blue-500 to-indigo-500 bg-clip-text text-transparent">Verify with Zero-Knowledge</span>
      </h1>
      
      <p class="mx-auto mb-10 max-w-2xl text-base md:text-lg text-slate-400 leading-relaxed">
        Experience SolvEx's micro-second latency matching engine with full cryptographic Proof of Solvency. 
        Enjoy the deep liquidity of a Centralized Exchange while guaranteeing your digital assets are mathematically secure.
      </p>

      <div class="flex flex-col sm:flex-row items-center justify-center gap-4">
        <button
          type="button"
          class="h-12 w-full sm:w-auto rounded-xl bg-linear-to-r from-cyan-500 to-blue-600 px-8 text-sm font-bold text-white shadow-lg shadow-cyan-500/20 transition-all hover:-translate-y-0.5 hover:shadow-cyan-500/40 focus:ring-2 focus:ring-cyan-500/50"
          onclick={() => router.navigate("/login")}
        >
          Start Trading
        </button>
        <button
          type="button"
          class="h-12 w-full sm:w-auto rounded-xl border border-slate-700 bg-slate-800/40 px-8 text-sm font-bold text-slate-300 transition-all hover:bg-slate-700 hover:text-white"
          onclick={() => { document.getElementById("markets-section")?.scrollIntoView({ behavior: 'smooth' }) }}
        >
          Explore Markets
        </button>
      </div>
    </div>
  </header>

  <!-- Core Features Container -->
  <section class="grid grid-cols-1 gap-4 md:grid-cols-3 md:gap-6">
    <!-- Feature 1 -->
    <div class="flex flex-col items-center rounded-2xl border border-slate-800 bg-slate-900/40 p-6 text-center transition hover:border-cyan-500/30 hover:bg-slate-800/60">
      <div class="mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-cyan-500/10 text-cyan-400">
        <svg xmlns="http://www.w3.org/2000/svg" class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M13 10V3L4 14h7v7l9-11h-7z" />
        </svg>
      </div>
      <h3 class="mb-2 text-base font-bold text-slate-100">Micro-Second Latency</h3>
      <p class="text-xs text-slate-400">Pure bare-metal Rust engine matching orders entirely in-memory for unmatched speed.</p>
    </div>
    
    <!-- Feature 2 -->
    <div class="flex flex-col items-center rounded-2xl border border-slate-800 bg-slate-900/40 p-6 text-center transition hover:border-emerald-500/30 hover:bg-slate-800/60">
      <div class="mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-emerald-500/10 text-emerald-400">
        <svg xmlns="http://www.w3.org/2000/svg" class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M9 12l2 2 4-4m5.618-4.016A11.955 11.955 0 0112 2.944a11.955 11.955 0 01-8.618 3.04A12.02 12.02 0 003 9c0 5.591 3.824 10.29 9 11.622 5.176-1.332 9-6.03 9-11.622 0-1.042-.133-2.052-.382-3.016z" />
        </svg>
      </div>
      <h3 class="mb-2 text-base font-bold text-slate-100">Zero-Knowledge Proofs</h3>
      <p class="text-xs text-slate-400">Merkle Sum Trees and Poseidon Hashes mathematically guarantee your funds are safu.</p>
    </div>

    <!-- Feature 3 -->
    <div class="flex flex-col items-center rounded-2xl border border-slate-800 bg-slate-900/40 p-6 text-center transition hover:border-purple-500/30 hover:bg-slate-800/60">
      <div class="mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-purple-500/10 text-purple-400">
        <svg xmlns="http://www.w3.org/2000/svg" class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4m0 5c0 2.21-3.582 4-8 4s-8-1.79-8-4" />
        </svg>
      </div>
      <h3 class="mb-2 text-base font-bold text-slate-100">Robust Persistence</h3>
      <p class="text-xs text-slate-400">Asynchronous PostgreSQL micro-batching guarantees data durability without blocking the main engine.</p>
    </div>
  </section>

  <!-- Markets Section -->
  <section id="markets-section" class="terminal-panel-strong p-5 md:p-8">
    <div class="mb-6 flex flex-wrap items-end justify-between gap-4 border-b border-slate-800 pb-4">
      <div>
        <h2 class="text-lg font-bold text-slate-100">Live Markets</h2>
        <p class="mt-1 text-sm text-slate-400">Real-time trading data powered by our high-frequency engine.</p>
      </div>
      <div class="flex items-center gap-2 text-xs font-medium text-slate-400 bg-slate-900/50 px-3 py-1.5 rounded-lg border border-slate-800">
        <span class="relative flex h-2 w-2">
          <span class="absolute inline-flex h-full w-full animate-ping rounded-full bg-emerald-400 opacity-75"></span>
          <span class="relative inline-flex h-2 w-2 rounded-full bg-emerald-500"></span>
        </span>
        Updated: <span class="mono ml-1 text-slate-300">{updatedAt || "--:--:--"}</span>
      </div>
    </div>

    <div class="overflow-x-auto pb-4">
      <table class="w-full min-w-175 border-collapse text-left">
        <thead>
          <tr class="border-b border-slate-800 text-[11px] font-semibold uppercase tracking-widest text-slate-500">
            <th class="py-4 pl-4 font-medium">Market</th>
            <th class="py-4 font-medium">Last Price</th>
            <th class="py-4 font-medium">24h Change</th>
            <th class="py-4 font-medium">24h Volume</th>
            <th class="py-4 pr-4 font-medium text-right">Action</th>
          </tr>
        </thead>
        <tbody class="divide-y divide-slate-800/50">
          {#if isLoading}
            <tr>
              <td colspan="5" class="py-12 text-center text-sm text-slate-400">
                <div class="flex flex-col items-center gap-3">
                  <div class="h-6 w-6 animate-spin rounded-full border-2 border-slate-600 border-t-cyan-500"></div>
                  <span>Syncing market feed...</span>
                </div>
              </td>
            </tr>
          {:else}
            {#each rows as row}
              <tr class="group transition-colors hover:bg-slate-800/30">
                <td class="py-4 pl-4">
                  <div class="flex items-center gap-3">
                    <img
                      src={row.iconUrl}
                      alt={`${row.symbol} icon`}
                      class="h-8 w-8 rounded-full ring-1 ring-slate-700 bg-slate-800 p-1"
                      loading="lazy"
                      onerror={(event) => {
                        const target = event.currentTarget as HTMLImageElement;
                        if (!target.src.endsWith('/icons/coins/default.svg')) {
                          target.src = '/icons/coins/default.svg';
                        }
                      }}
                    />
                    <span class="mono text-sm font-bold text-slate-100">{row.symbol.replace('_', '/')}</span>
                  </div>
                </td>
                <td class="py-4">
                  <span class="mono text-sm font-medium text-slate-200">${row.price}</span>
                </td>
                <td class="py-4">
                  <div class="inline-flex items-center gap-1 rounded bg-slate-900/50 px-2 py-1 mono text-xs font-semibold {row.isPositive ? 'text-emerald-400 ring-1 ring-emerald-500/20' : 'text-rose-400 ring-1 ring-rose-500/20'}">
                    {#if row.isPositive}
                      <svg xmlns="http://www.w3.org/2000/svg" class="h-3 w-3" viewBox="0 0 20 20" fill="currentColor">
                        <path fill-rule="evenodd" d="M12 7a1 1 0 110-2h5a1 1 0 011 1v5a1 1 0 11-2 0V8.414l-4.293 4.293a1 1 0 01-1.414 0L8 10.414l-4.293 4.293a1 1 0 01-1.414-1.414l5-5a1 1 0 011.414 0L11 10.586 14.586 7H12z" clip-rule="evenodd" />
                      </svg>
                    {:else}
                      <svg xmlns="http://www.w3.org/2000/svg" class="h-3 w-3" viewBox="0 0 20 20" fill="currentColor">
                        <path fill-rule="evenodd" d="M12 13a1 1 0 100 2h5a1 1 0 001-1V9a1 1 0 10-2 0v2.586l-4.293-4.293a1 1 0 00-1.414 0L8 9.586 3.707 5.293a1 1 0 00-1.414 1.414l5 5a1 1 0 001.414 0L11 9.414 14.586 13H12z" clip-rule="evenodd" />
                      </svg>
                    {/if}
                    {row.changePct24h}
                  </div>
                </td>
                <td class="py-4">
                  <span class="mono text-sm text-slate-400">{row.volume24h} USDT</span>
                </td>
                <td class="py-4 pr-4 text-right">
                  <button
                    type="button"
                    class="rounded-lg bg-cyan-500/10 px-4 py-2 text-xs font-bold tracking-wide text-cyan-400 ring-1 ring-inset ring-cyan-500/20 transition-all hover:bg-cyan-500 hover:text-white hover:ring-cyan-500 group-hover:-translate-y-0.5"
                    onclick={() => openTerminal(row.symbol)}
                  >
                    Trade
                  </button>
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </div>
  </section>
</section>

<style>
  .landing-hero {
    background-image: radial-gradient(circle at 50% 0%, rgba(14, 165, 233, 0.1) 0%, transparent 60%);
  }
</style>
