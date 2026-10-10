<script lang="ts">
  import { authState } from '../../stores/authStore';
  import { SUPPORTED_ASSET_SYMBOLS } from '../../lib/marketMeta';
  import { validateProofJson, runWasmVerification } from '../../lib/zkp-verify';
  import type { ProofPayload, VerifyStatus } from '../../lib/zkp-types';

  let proofData = $state("");
  let status = $state<VerifyStatus>("idle");
  let errorMsg = $state("");
  let assetFilter = $state("USDT");
  let dropActive = $state(false);
  let revision = 0;

  function resetAssessment() {
    revision += 1;
    status = "idle";
    errorMsg = "";
  }

  async function fetchProofFromServer() {
    resetAssessment();
    const current = revision;
    status = "fetching";
    try {
      const response = await fetch(`/api/zkp/proof?asset=${encodeURIComponent(assetFilter)}`, {
        headers: { "x-user-id": String($authState.userId ?? "") }
      });
      if (!response.ok) throw new Error(`Server returned ${response.status}`);
      const data = await response.json();
      if (current !== revision) return;
      proofData = JSON.stringify(data, null, 2);
      status = "idle";
    } catch (error) {
      if (current !== revision) return;
      status = "error";
      errorMsg = error instanceof Error ? error.message : "Proof request failed.";
    }
  }

  async function parseAndSetProofFile(file: File) {
    resetAssessment();
    const current = revision;
    if (file.size > 1_048_576) throw new Error("Proof file exceeds 1 MiB.");
    const text = await file.text();
    if (current === revision) proofData = text;
  }

  async function handleFileUpload(event: Event) {
    const input = event.target as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    try { await parseAndSetProofFile(file); }
    catch (error) {
      status = "error";
      errorMsg = error instanceof Error ? error.message : "File read failed.";
    }
  }

  async function handleDrop(event: DragEvent) {
    event.preventDefault();
    dropActive = false;
    const file = event.dataTransfer?.files?.[0];
    if (!file) return;
    try { await parseAndSetProofFile(file); }
    catch (error) {
      status = "error";
      errorMsg = error instanceof Error ? error.message : "File read failed.";
    }
  }

  async function verifyProof() {
    const parsed = validateProofJson(proofData);
    if (!parsed.ok) {
      status = "invalid";
      errorMsg = parsed.error;
      return;
    }
    const current = revision;
    status = "verifying";
    try {
      const result = await runWasmVerification(parsed.payload);
      if (current !== revision) return;
      status = result.status;
      errorMsg = result.reason;
    } catch {
      if (current !== revision) return;
      status = "unavailable";
      errorMsg = "Local verifier unavailable; no cryptographic result was produced.";
    }
  }
</script>

<section class="terminal-panel-strong p-4 sm:p-5 h-full flex flex-col relative overflow-hidden zk-panel">
  <div class="mb-4 flex items-start justify-between gap-3">
    <div>
      <h2 class="text-sm font-semibold tracking-wide text-slate-100 uppercase">
        User Verification
      </h2>
    </div>
    <span class="mono rounded border border-cyan-500/25 bg-cyan-500/10 px-2 py-0.5 text-[10px] text-cyan-200">
      Prototype — unverified
    </span>
  </div>

  <div class="flex-1 flex flex-col space-y-4">
    <div class="grid gap-2 md:grid-cols-[100px_1fr_auto] md:items-center">
      <select
        bind:value={assetFilter}
        class="rounded border border-slate-700/80 bg-slate-900/80 px-2 py-2 text-xs text-slate-200 outline-none focus:border-cyan-500/50 cursor-pointer"
      >
        <option value="USDT">USDT</option>
        {#each SUPPORTED_ASSET_SYMBOLS as symbol}
          <option value={symbol}>{symbol}</option>
        {/each}
      </select>

      <button
        type="button"
        disabled={status === "fetching" || status === "verifying"}
        onclick={fetchProofFromServer}
        class="h-9 rounded border border-cyan-500/30 bg-cyan-500/12 px-3 text-xs font-semibold tracking-wider text-cyan-200 transition hover:bg-cyan-500/20 active:scale-[0.98] disabled:opacity-50 disabled:pointer-events-none uppercase"
      >
        {status === "fetching" ? "Fetching Proof..." : "Fetch From API"}
      </button>

      <label class="inline-flex cursor-pointer items-center justify-center h-9 rounded border border-slate-700 bg-slate-800/90 px-3 text-[10px] uppercase tracking-wider text-slate-300 transition hover:bg-slate-700">
        Upload JSON
        <input type="file" accept=".json,.txt" class="hidden" onchange={handleFileUpload} />
      </label>
    </div>

    <div class="flex-1 space-y-2 flex flex-col">
      <div class="flex items-center justify-between">
        <label class="block text-[11px] font-medium tracking-widest text-slate-400 uppercase" for="proof-data">Proof Payload</label>
        <span class="mono text-[10px] text-slate-500">Drag and drop supported</span>
      </div>

      <textarea
        id="proof-data"
        bind:value={proofData}
        oninput={resetAssessment}
        ondragenter={(e) => {
          e.preventDefault();
          dropActive = true;
        }}
        ondragover={(e) => {
          e.preventDefault();
          dropActive = true;
        }}
        ondragleave={(e) => {
          e.preventDefault();
          dropActive = false;
        }}
        ondrop={handleDrop}
        placeholder={'Fetch from API, paste JSON payload, or upload proof file.\n\nRequired fields:\n  user_id, root_hash, snark.proof, snark.public_inputs'}
        class="mono mt-1 block w-full flex-1 min-h-36 resize-none rounded-xl border bg-slate-950/70 px-3 py-2 text-xs text-slate-300 outline-none transition hide-scrollbar placeholder:text-slate-600 {dropActive ? 'border-cyan-400/60 ring-1 ring-cyan-400/40' : 'border-slate-700/80 focus:border-cyan-500/50'}"
      ></textarea>
    </div>

    <div class="grid gap-3 md:grid-cols-[1.25fr_1fr]">
      <div class="status-board p-3 rounded-xl border {
        status === 'unverified' || status === 'unsupported' || status === 'unavailable' ? 'border-amber-500/40 text-amber-300' :
        status === 'invalid'   ? 'border-rose-500/40 text-rose-300' :
        status === 'error'     ? 'border-orange-500/40 text-orange-300' :
        status === 'verifying' ? 'border-cyan-500/40 text-cyan-300' :
        status === 'fetching'  ? 'border-sky-500/40 text-sky-300' :
        'border-slate-800 text-slate-400'
      }">
        <p class="mono text-[11px] uppercase tracking-[0.15em] font-semibold">
          {#if status === 'idle'}Status: Ready
          {:else if status === 'fetching'}Status: Fetching proof
          {:else if status === 'verifying'}Status: Verifying
          {:else if status === 'unverified'}Status: Unverified (legacy){:else if status === 'unsupported'}Status: Unsupported{:else if status === 'unavailable'}Status: Verifier unavailable
          {:else if status === 'invalid'}Status: Invalid structure
          {:else if status === 'error'}Status: Error
          {/if}
        </p>

        {#if errorMsg}
          <p class="mt-2 text-xs leading-relaxed text-orange-300/95">{errorMsg}</p>
        {/if}
      </div>

      {#if proofData}
        {@const parsed = (() => {
          try {
            return JSON.parse(proofData) as ProofPayload;
          } catch {
            return null;
          }
        })()}
        <div class="rounded-xl border border-slate-700/70 bg-slate-950/55 p-3 text-xs text-slate-300/90">
            <p class="mono uppercase tracking-[0.14em] text-slate-400">Unverified prototype</p>
            {#if parsed}
              <p class="mt-2">User proof loaded</p>
              <p class="mt-1">Leaf Balance: <span class="mono text-slate-100">{parsed.leaf_balance ?? '-'}</span></p>
              <p class="mt-1">SNARK: <span class="mono text-slate-100">{parsed.snark?.scheme ?? 'unknown'}</span></p>
            {:else}
              <p class="mt-2 text-slate-400">No user proof loaded.</p>
            {/if}
        </div>
      {/if}
    </div>

    <button
      type="button"
      onclick={verifyProof}
      disabled={status === 'verifying' || status === 'fetching'}
      class="w-full h-10 rounded-xl border border-cyan-500/35 bg-cyan-500/18 text-sm font-semibold tracking-wider text-cyan-200 transition hover:bg-cyan-500/26 active:scale-[0.98] disabled:opacity-50 disabled:active:scale-100 uppercase"
    >
      {status === 'verifying'
        ? 'Inspecting...'
        : 'Inspect Proof Status'}
    </button>
  </div>
</section>

<style>
  .zk-panel {
    background:
      radial-gradient(740px 220px at 100% -20%, rgba(34, 211, 238, 0.1), transparent 60%),
      color-mix(in srgb, #020617 84%, transparent);
  }

  .status-board {
    background: rgba(2, 6, 23, 0.6);
  }

  .hide-scrollbar::-webkit-scrollbar {
    width: 6px;
  }
  .hide-scrollbar::-webkit-scrollbar-track {
    background: transparent;
  }
  .hide-scrollbar::-webkit-scrollbar-thumb {
    background-color: rgb(30 41 59);
    border-radius: 20px;
  }
</style>

