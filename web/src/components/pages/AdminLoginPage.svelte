<script lang="ts">
  import { router } from "../../stores/routerStore";
  import { loginAdmin } from "../../stores/adminAuthStore";

  let userId = $state("");
  let errorMsg = $state("");
  let isSubmitting = $state(false);

  async function handleLogin(e: Event) {
    e.preventDefault();
    errorMsg = "";
    isSubmitting = true;

    try {
      const success = await loginAdmin(userId.trim());
      if (success) {
        router.navigate("/admin");
      } else {
        errorMsg = "Server policy denied this declared ID or demo mode is disabled.";
      }
    } catch (err: any) {
      errorMsg = err.message || "An error occurred";
    } finally {
      isSubmitting = false;
    }
  }
</script>

<div class="mx-auto mt-16 max-w-md">
  <div class="terminal-panel p-6 shadow-2xl">
    <div class="mb-6 text-center">
      <h2 class="mono text-xl tracking-wider text-sky-400">
        DEMO ADMIN ACCESS
      </h2>
      <p class="mt-2 text-sm text-slate-400">
        Development only. The server checks the role; anyone can impersonate a declared ID.
      </p>
    </div>

    <form onsubmit={handleLogin} class="space-y-4">
      {#if errorMsg}
        <div class="rounded-md border border-rose-500/30 bg-rose-500/10 p-3 text-sm text-rose-400">
          {errorMsg}
        </div>
      {/if}

      <div>
        <label for="admin_username" class="mb-1.5 block text-xs font-medium text-slate-300">
          Declared admin user ID
        </label>
        <input
          id="admin_username"
          type="text"
          bind:value={userId}
          class="w-full rounded bg-slate-900 border border-slate-700 px-3 py-2 text-sm text-slate-100 placeholder-slate-500 outline-none focus:border-sky-500 focus:ring-1 focus:ring-sky-500/50"
          placeholder="Positive user ID configured on the server"
          required
        />
      </div>

      <button
        type="submit"
        disabled={isSubmitting}
        class="mt-6 w-full rounded bg-sky-500 px-4 py-2.5 text-sm font-semibold text-white shadow-lg shadow-sky-500/20 transition-all hover:bg-sky-400 disabled:opacity-50"
      >
        {isSubmitting ? "Checking server policy..." : "Open demo admin"}
      </button>
    </form>
  </div>
</div>
