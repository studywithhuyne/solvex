<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { router } from "../../stores/routerStore";
  import { logoutAdmin } from "../../stores/adminAuthStore";
  import {
    fetchAdminMetrics,
    fetchTreasuryMetrics,
    fetchAdminAssets,
    addAsset,
    fetchAdminUsers,
    suspendUser,
    adminTreasuryDeposit,
    adminTreasuryWithdraw,
    haltMarket,
    triggerZkpSnapshot,
    fetchZkpHistory,
    type AdminMetrics,
    type TreasuryMetrics,
    type AdminAssetDto,
    type AdminUserDto,
    type ZkSnapshotDto
  } from "../../lib/api/client";

  // Dashboard state
  let metrics = $state<AdminMetrics | null>(null);
  let treasury = $state<TreasuryMetrics | null>(null);

  // ZKP State
  let zkpHistory = $state<ZkSnapshotDto[]>([]);
  let selectedSnapshot = $state<ZkSnapshotDto | null>(null);

  // Assets state
  let assets = $state<AdminAssetDto[]>([]);
  let newAssetSymbol = $state("");
  let newAssetName = $state("");

  // Users state
  let users = $state<AdminUserDto[]>([]);
  
  // UI Tab state
  let activeTab = $state<"dashboard" | "assets" | "users" | "zkp">("dashboard");

  // Notifications
  let message = $state<string | null>(null);
  let treasuryAmount = $state("10000");
  let treasuryPending = $state(false);
  const DASHBOARD_REFRESH_MS = 2000;
  let dashboardRefreshTimer: ReturnType<typeof setInterval> | null = null;

  function formatDecimalString(value: string, fractionDigits: number = 3): string {
    const raw = value.trim();
    if (!raw) return "0";

    const sign = raw.startsWith("-") ? "-" : "";
    const normalized = sign ? raw.slice(1) : raw;
    const [intPartRaw, fracPartRaw = ""] = normalized.split(".");

    // Fallback for unexpected payload format.
    if (!/^\d+$/.test(intPartRaw || "0") || (fracPartRaw && !/^\d+$/.test(fracPartRaw))) {
      return value;
    }

    const intPart = (intPartRaw || "0").replace(/^0+(?=\d)/, "");
    const groupedInt = intPart.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
    const fracPart = fracPartRaw.slice(0, Math.max(0, fractionDigits)).replace(/0+$/, "");

    return fracPart.length > 0 ? `${sign}${groupedInt}.${fracPart}` : `${sign}${groupedInt}`;
  }

  function formatSolvencyRatio(value: string): string {
    if (value === "infinity") return "infinity";
    return formatDecimalString(value, 4);
  }

  function stopDashboardRefresh() {
    if (dashboardRefreshTimer) {
      clearInterval(dashboardRefreshTimer);
      dashboardRefreshTimer = null;
    }
  }

  function startDashboardRefresh() {
    if (dashboardRefreshTimer) return;
    dashboardRefreshTimer = setInterval(() => {
      if (activeTab === "dashboard") {
        void loadDashboard();
      }
    }, DASHBOARD_REFRESH_MS);
  }

  async function loadDashboard() {
    try {
      metrics = await fetchAdminMetrics();
      treasury = await fetchTreasuryMetrics();
    } catch (e) {
      console.error(e);
    }
  }

  async function handleAddAsset() {
    if (!newAssetSymbol || !newAssetName) {
      alert("Please fill in both symbol and name.");
      return;
    }
    try {
      await addAsset(newAssetSymbol, newAssetName);
      newAssetSymbol = "";
      newAssetName = "";
      await loadAssets(); // Refresh the list
      message = "Asset added successfully.";
      setTimeout(() => (message = null), 3000);
    } catch (e) {
      console.error(e);
      alert("Failed to add asset.");
    }
  }

  async function loadAssets() {
    try {
      assets = await fetchAdminAssets();
    } catch (e) {
      console.error(e);
    }
  }

  async function loadUsers() {
    try {
      users = await fetchAdminUsers();
    } catch (e) {
      console.error(e);
    }
  }

  async function loadZkp() {
    try {
      zkpHistory = await fetchZkpHistory();
    } catch (e) {
      console.error(e);
    }
  }

  onMount(() => {
    loadDashboard();
    startDashboardRefresh();
  });

  onDestroy(() => {
    stopDashboardRefresh();
  });

  $effect(() => {
    if (activeTab === "dashboard") {
      loadDashboard();
      startDashboardRefresh();
    } else {
      stopDashboardRefresh();
    }
    if (activeTab === "assets") loadAssets();
    if (activeTab === "users") loadUsers();
    if (activeTab === "zkp") loadZkp();
  });

  async function handleSuspend(userId: number) {
    if (!confirm(`Are you sure you want to suspend user ${userId}?`)) return;
    try {
      await suspendUser(userId);
      message = `User ${userId} suspended successfully.`;
      loadUsers();
      setTimeout(() => message = null, 3000);
    } catch (e: any) {
      alert("Error: " + e.message);
    }
  }

  async function handleHaltMarket(symbol: string) {
    if (!confirm(`Are you sure you want to HALT trading for ${symbol}?`)) return;
    try {
      await haltMarket(symbol);
      message = `Market ${symbol} halted successfully.`;
      await loadAssets();
      setTimeout(() => message = null, 3000);
    } catch (e: any) {
      alert("Error: " + e.message);
    }
  }

  async function handleZkpSnapshot() {
    if (!confirm("Create a simulated demo record? This will not generate a cryptographic snapshot.")) return;
    try {
      await triggerZkpSnapshot();
      message = `Simulated record created; no verified snapshot was produced.`;
      loadZkp();
      setTimeout(() => message = null, 3000);
    } catch (e: any) {
      alert("Error: " + e.message);
    }
  }

  async function handleTreasuryDeposit() {
    const amount = treasuryAmount.trim();
    if (!amount) {
      alert("Please enter an amount.");
      return;
    }

    treasuryPending = true;
    try {
      await adminTreasuryDeposit(amount);
      await loadDashboard();
      message = `Exchange capital deposited: ${amount} USDT`;
      setTimeout(() => (message = null), 3000);
    } catch (e: any) {
      alert("Error: " + (e?.message ?? "deposit failed"));
    } finally {
      treasuryPending = false;
    }
  }

  async function handleTreasuryWithdraw() {
    const amount = treasuryAmount.trim();
    if (!amount) {
      alert("Please enter an amount.");
      return;
    }

    treasuryPending = true;
    try {
      await adminTreasuryWithdraw(amount);
      await loadDashboard();
      message = `Exchange capital withdrawn: ${amount} USDT`;
      setTimeout(() => (message = null), 3000);
    } catch (e: any) {
      alert("Error: " + (e?.message ?? "withdraw failed"));
    } finally {
      treasuryPending = false;
    }
  }
</script>

<div class="space-y-6 max-w-6xl mx-auto">
  <!-- Header -->
  <div class="px-4 py-3 bg-slate-900 border border-slate-800 rounded-xl flex items-center justify-between">
    <div>
      <h1 class="text-sm font-semibold tracking-widest text-slate-100 uppercase">Admin Dashboard</h1>
      <p class="text-xs text-slate-500 mt-1">Management and Monitoring</p>
    </div>
    
<div class="flex items-center gap-4">
      <div class="flex gap-2">
        <button
          class="px-3 py-1.5 text-xs font-semibold rounded-md border {activeTab === 'dashboard' ? 'bg-sky-500/20 text-sky-400 border-sky-500/50' : 'bg-slate-800 text-slate-400 border-slate-700'}"
          onclick={() => activeTab = 'dashboard'}>Dashboard</button>
        <button
          class="px-3 py-1.5 text-xs font-semibold rounded-md border {activeTab === 'assets' ? 'bg-sky-500/20 text-sky-400 border-sky-500/50' : 'bg-slate-800 text-slate-400 border-slate-700'}"
          onclick={() => activeTab = 'assets'}>Markets & Assets</button>
        <button
          class="px-3 py-1.5 text-xs font-semibold rounded-md border {activeTab === 'users' ? 'bg-sky-500/20 text-sky-400 border-sky-500/50' : 'bg-slate-800 text-slate-400 border-slate-700'}"
          onclick={() => activeTab = 'users'}>Users</button>
        <button
          class="px-3 py-1.5 text-xs font-semibold rounded-md border {activeTab === 'zkp' ? 'bg-sky-500/20 text-sky-400 border-sky-500/50' : 'bg-slate-800 text-slate-400 border-slate-700'}"
          onclick={() => activeTab = 'zkp'}>ZKP Audit</button>
      </div>

      <div class="h-6 w-px bg-slate-700"></div>

      <button
        class="px-3 py-1.5 text-xs font-semibold rounded-md border bg-slate-800 text-slate-300 border-slate-700 hover:border-slate-500 transition-colors"
        onclick={() => { logoutAdmin(); router.navigate("/admin/login"); }}
      >
        Logout
      </button>
    </div>
  </div>

  {#if message}
    <div class="bg-emerald-500/20 border border-emerald-500/50 text-emerald-400 px-4 py-2 rounded-lg text-sm text-center">
      {message}
    </div>
  {/if}

  <!-- Tab Content -->
  {#if activeTab === "dashboard"}
    <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
      <div class="terminal-panel p-5">
        <h2 class="text-xs font-medium text-slate-400 mb-4 uppercase tracking-widest border-b border-slate-800 pb-2">Exchange Metrics</h2>
        {#if metrics}
          <div class="space-y-4">
            <div class="flex justify-between items-center bg-slate-900/50 p-3 rounded border border-slate-800/50">
              <span class="text-sm text-slate-400">24h Volume (USDT)</span>
              <span class="text-sm font-bold text-sky-400 mono">${formatDecimalString(metrics.volume_24h_usdt)}</span>
            </div>
            <div class="flex justify-between items-center bg-slate-900/50 p-3 rounded border border-slate-800/50">
              <span class="text-sm text-slate-400">Total Users</span>
              <span class="text-sm font-bold text-slate-200 mono">{metrics.total_users}</span>
            </div>
            <div class="flex justify-between items-center bg-slate-900/50 p-3 rounded border border-slate-800/50">
              <span class="text-sm text-slate-400">Active Orders</span>
              <span class="text-sm font-bold text-slate-200 mono">{metrics.active_orders}</span>
            </div>
          </div>
        {:else}
          <p class="text-sm text-slate-600">Loading metrics...</p>
        {/if}
      </div>

      <div class="terminal-panel p-5">
        <h2 class="text-xs font-medium text-slate-400 mb-4 uppercase tracking-widest border-b border-slate-800 pb-2">Treasury & Solvency (USDT)</h2>
        {#if treasury}
          <div class="space-y-4">
            <div class="flex justify-between items-center bg-slate-900/50 p-3 rounded border border-slate-800/50">
              <span class="text-sm text-slate-400">Exchange Base Capital</span>
              <span class="text-sm font-bold text-sky-400 mono">${formatDecimalString(treasury.exchange_capital)}</span>
            </div>
            <div class="flex justify-between items-center bg-slate-900/50 p-3 rounded border border-slate-800/50">
              <span class="text-sm text-slate-400">Exchange Revenue (Trading Fees)</span>
              <span class="text-sm font-bold text-amber-300 mono">${formatDecimalString(treasury.exchange_revenue)}</span>
            </div>
            <div class="flex justify-between items-center bg-slate-900/50 p-3 rounded border border-slate-800/50">
              <span class="text-sm text-slate-400">Total Liabilities</span>
              <span class="text-sm font-bold text-rose-400 mono">${formatDecimalString(treasury.total_user_liabilities)}</span>
            </div>
            <div class="flex justify-between items-center bg-slate-900/50 p-3 rounded border border-slate-800/50 mt-2">
              <span class="text-sm text-slate-400 font-medium">Total Exchange Assets (Capital + Revenue)</span>
              <span class="text-sm font-bold text-emerald-400 mono">${formatDecimalString(treasury.total_exchange_funds)}</span>
            </div>
            <div class="flex justify-between items-center bg-slate-900/50 p-3 rounded border border-slate-800/50">
              <span class="text-sm text-slate-400">Solvency Ratio</span>
              <span class="text-sm font-bold text-sky-400 mono">{formatSolvencyRatio(treasury.solvency_ratio)}</span>
            </div>

            <div class="mt-2 rounded border border-slate-800/50 bg-slate-900/40 p-3">
              <p class="mb-2 text-[10px] uppercase tracking-widest text-slate-500">Exchange Capital Top-up / Withdraw</p>
              <div class="flex flex-col gap-2 sm:flex-row">
                <input
                  type="text"
                  bind:value={treasuryAmount}
                  placeholder="Amount in USDT"
                  class="h-9 flex-1 rounded border border-slate-700 bg-slate-950 px-3 text-sm text-slate-200 focus:border-sky-500 focus:outline-none"
                />
                <button
                  class="h-9 rounded border border-emerald-500/40 bg-emerald-500/20 px-3 text-xs font-semibold uppercase tracking-wide text-emerald-300 disabled:opacity-60"
                  onclick={handleTreasuryDeposit}
                  disabled={treasuryPending}
                >
                  Deposit
                </button>
                <button
                  class="h-9 rounded border border-rose-500/40 bg-rose-500/20 px-3 text-xs font-semibold uppercase tracking-wide text-rose-300 disabled:opacity-60"
                  onclick={handleTreasuryWithdraw}
                  disabled={treasuryPending}
                >
                  Withdraw
                </button>
              </div>
            </div>
          </div>
        {:else}
          <p class="text-sm text-slate-600">Loading treasury...</p>
        {/if}
      </div>
    </div>
  {/if}

  {#if activeTab === "assets"}
    <div class="terminal-panel p-5">
        <h2 class="text-xs font-medium text-slate-400 mb-4 uppercase tracking-widest border-b border-slate-800 pb-2">Supported Assets & Markets</h2>
        <table class="w-full text-left text-sm mb-6">
          <thead class="text-xs text-slate-500 uppercase bg-slate-900/80">
            <tr>
              <th class="py-2 px-3">Symbol</th>
              <th class="py-2 px-3">Name</th>
              <th class="py-2 px-3">Status</th>
              <th class="py-2 px-3 text-right">Actions</th>
            </tr>
          </thead>
          <tbody>
            {#each assets as asset}
            <tr class="border-b border-slate-800">
              <td class="py-2 px-3 font-medium text-slate-200">{asset.symbol}</td>
              <td class="py-2 px-3 text-slate-400">{asset.name}</td>
              <td class="py-2 px-3">
                <span class={asset.is_active ? 'text-emerald-400' : 'text-slate-500'}>
                  {asset.is_active ? 'Active' : 'Inactive'}
                </span>
              </td>
              <td class="py-2 px-3 text-right">
                {#if asset.is_active && asset.symbol !== "USDT"}
                  <button class="text-xs bg-rose-500/20 text-rose-400 border border-rose-500/30 px-2 py-1 rounded hover:bg-rose-500/30"
                          onclick={() => handleHaltMarket(`${asset.symbol}_USDT`)}>
                    Halt Market
                  </button>
                {/if}
              </td>
            </tr>
            {/each}
          </tbody>
        </table>
        
        <div class="bg-slate-900 border border-slate-800 rounded-lg p-4">
          <h3 class="text-xs text-slate-400 uppercase tracking-widest mb-3">Add New Asset</h3>
          <div class="flex gap-3">
              <input type="text" placeholder="Symbol (e.g. ADA)" bind:value={newAssetSymbol} class="bg-slate-950 border border-slate-700 rounded px-3 py-1.5 text-sm w-32 focus:outline-none focus:border-sky-500 text-white" />
              <input type="text" placeholder="Name" bind:value={newAssetName} class="bg-slate-950 border border-slate-700 rounded px-3 py-1.5 text-sm w-48 focus:outline-none focus:border-sky-500 text-white" />
              <button onclick={handleAddAsset} class="bg-sky-600 hover:bg-sky-500 text-white px-4 py-1.5 rounded text-sm font-medium transition-colors">Add</button>
          </div>
        </div>
    </div>
  {/if}

  {#if activeTab === "users"}
    <div class="terminal-panel p-5">
      <h2 class="text-xs font-medium text-slate-400 mb-4 uppercase tracking-widest border-b border-slate-800 pb-2">User Management</h2>
      <table class="w-full text-left text-sm">
        <thead class="text-xs text-slate-500 uppercase bg-slate-900/80">
          <tr>
            <th class="py-2 px-3">User ID</th>
            <th class="py-2 px-3">Username</th>
            <th class="py-2 px-3">Status</th>
            <th class="py-2 px-3 text-right">Actions</th>
          </tr>
        </thead>
        <tbody>
          {#each users as user}
          <tr class="border-b border-slate-800">
            <td class="py-2 px-3 font-medium text-slate-400">{user.user_id}</td>
            <td class="py-2 px-3 text-slate-200">{user.username}</td>
            <td class="py-2 px-3">
              <span class={user.is_suspended ? 'text-rose-400' : 'text-emerald-400'}>
                {user.is_suspended ? 'Suspended' : 'Active'}
              </span>
            </td>
            <td class="py-2 px-3 text-right">
              {#if !user.is_suspended}
                <button class="text-xs bg-rose-500/20 text-rose-400 border border-rose-500/30 px-2 py-1 rounded hover:bg-rose-500/30"
                        onclick={() => handleSuspend(user.user_id)}>
                  Suspend
                </button>
              {/if}
            </td>
          </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

  {#if activeTab === "zkp"}
    <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
      <div class="terminal-panel p-5 text-center flex flex-col justify-center">
        <h2 class="text-xs font-medium text-slate-400 mb-4 uppercase tracking-widest border-b border-slate-800 pb-2">ZKP Audit Operations</h2>
        <div class="py-8">
          <p class="text-sm text-slate-300 mb-6 max-w-sm mx-auto leading-relaxed">
            This action records demo activity only. It does not capture an immutable ledger snapshot, construct a cryptographic root or prove solvency.
          </p>
          <button class="bg-indigo-600 hover:bg-indigo-500 text-white px-6 py-2.5 rounded-lg text-sm font-semibold tracking-wide transition-all shadow-[0_0_15px_rgba(79,70,229,0.3)] hover:shadow-[0_0_20px_rgba(79,70,229,0.5)]"
            onclick={handleZkpSnapshot}>
            Create Simulated Record
          </button>
        </div>
      </div>

      <div class="terminal-panel p-5">
        <h2 class="text-xs font-medium text-slate-400 mb-4 uppercase tracking-widest border-b border-slate-800 pb-2">Simulated / Unverified History</h2>
        <div class="space-y-3 max-h-100 overflow-y-auto pr-2 custom-scrollbar">
          {#each zkpHistory as snap}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div 
              class="bg-slate-900 border border-slate-800 rounded py-3 px-4 text-sm text-slate-300 cursor-pointer hover:border-sky-500/50 hover:bg-slate-800/80 transition-all flex flex-col gap-2 relative overflow-hidden group"
              onclick={() => selectedSnapshot = snap}
            >
              <div class="flex items-center justify-between">
                <span class="text-sky-400 font-semibold mono text-xs">{snap.snapshot_id}</span>
                <span class="text-[10px] text-slate-500">{new Date(snap.created_at).toLocaleString()}</span>
              </div>
              <div class="flex items-center justify-between border-t border-slate-800/50 pt-2">
                <span class="text-xs text-slate-400">Users observed: <span class="text-slate-200">{snap.users_observed}</span> · {snap.snapshot_kind} · unverified</span>
                <span class="text-xs text-indigo-400 truncate max-w-37.5 font-mono">{snap.root_hash}</span>
              </div>
            </div>
          {:else}
             <p class="text-sm text-slate-500 italic text-center p-4">No snapshots recorded yet.</p>
          {/each}
        </div>
      </div>
    </div>

    <!-- Snapshot Details Modal -->
    {#if selectedSnapshot}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4" onclick={() => selectedSnapshot = null}>
        <div class="bg-slate-900 border border-slate-600 rounded-xl shadow-2xl max-w-2xl w-full p-6 relative" onclick={(e) => e.stopPropagation()}>
          <!-- svelte-ignore a11y_consider_explicit_label -->
          <button class="absolute top-4 right-4 text-slate-400 hover:text-white" onclick={() => selectedSnapshot = null}>
            <svg xmlns="http://www.w3.org/2000/svg" class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>

          <h3 class="text-lg font-bold text-slate-100 mb-4 tracking-wide uppercase">Snapshot Details</h3>
          
          <div class="space-y-4 text-sm bg-slate-950 p-5 rounded-lg border border-slate-800">
            <div>
              <p class="text-slate-500 text-[10px] uppercase tracking-widest mb-1">Snapshot ID</p>
              <p class="text-sky-400 font-mono font-medium text-base">{selectedSnapshot.snapshot_id}</p>
            </div>
            
            <div class="grid grid-cols-2 gap-4">
              <div>
                <p class="text-slate-500 text-[10px] uppercase tracking-widest mb-1">Execution Time (Local)</p>
                <p class="text-slate-200">{new Date(selectedSnapshot.created_at).toLocaleString()}</p>
              </div>
              <div>
                <p class="text-slate-500 text-[10px] uppercase tracking-widest mb-1">Users Observed (Not Verified Inclusion)</p>
                <p class="text-slate-200 font-mono">{selectedSnapshot.users_observed}</p>
              </div>
            </div>

            <div>
              <p class="text-slate-500 text-[10px] uppercase tracking-widest mb-1">Untrusted Legacy / Simulated Value — {selectedSnapshot.snapshot_kind}</p>
              <div class="bg-slate-900 p-3 rounded border border-slate-800 text-indigo-400 font-mono break-all text-sm leading-relaxed">
                {selectedSnapshot.root_hash}
              </div>
            </div>
            
            <div>
              <p class="text-slate-500 text-[10px] uppercase tracking-widest mb-1">Raw Timestamp (UTC)</p>
              <p class="text-slate-400 text-xs font-mono">{selectedSnapshot.created_at}</p>
            </div>
          </div>
          
          <div class="mt-6 flex justify-end">
             <button class="px-5 py-2 bg-slate-800 hover:bg-slate-700 text-white rounded transition-colors text-sm font-medium" onclick={() => selectedSnapshot = null}>
               Close Details
             </button>
          </div>
        </div>
      </div>
    {/if}
{/if}
</div>

