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

export function deviceId(): string {
  let id = localStorage.getItem(DEVICE);
  if (!id) {
    id = `web-${crypto.randomUUID()}`;
    localStorage.setItem(DEVICE, id);
  }
  return id;
}
