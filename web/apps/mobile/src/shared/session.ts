// Access token and device ID kept in browser storage.
const TOKEN = "saltation.token";
const DEVICE = "saltation.device";

export function getToken(): string | undefined {
  return sessionStorage.getItem(TOKEN) ?? undefined;
}

export function setToken(token: string | undefined): void {
  if (token) sessionStorage.setItem(TOKEN, token);
  else sessionStorage.removeItem(TOKEN);
}

/** A random hex ID. crypto.getRandomValues works outside secure contexts;
 * crypto.randomUUID does not, so plain-HTTP origins (LAN, tests) would fail. */
export function randomHex(bytes: number): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (b) => b.toString(16).padStart(2, "0")).join("");
}

export function deviceId(): string {
  let id = localStorage.getItem(DEVICE);
  if (!id) {
    id = `web-${randomHex(16)}`;
    localStorage.setItem(DEVICE, id);
  }
  return id;
}
