import { writable } from "svelte/store";

export const adminAuthState = writable<{ isLoggedIn: boolean; userId: string | null }>({
  isLoggedIn: false,
  userId: null,
});

// The server owns demo roles. Declared IDs are still impersonable.
export const loginAdmin = async (userId: string) => {
  if (!/^[1-9]\d*$/.test(userId)) return false;
  const response = await fetch("/api/admin/metrics", { headers: { "x-user-id": userId } });
  adminAuthState.set({ isLoggedIn: response.ok, userId: response.ok ? userId : null });
  return response.ok;
};

export const logoutAdmin = () => {
  adminAuthState.set({ isLoggedIn: false, userId: null });
};

export const bootstrapAdminAuth = () => {
  // Local storage must never authorize a server-owned role.
  logoutAdmin();
};

