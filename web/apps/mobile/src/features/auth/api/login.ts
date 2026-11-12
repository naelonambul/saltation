import { bank, errorMessage } from "../../../shared/api";
import { deviceId, setToken } from "../../../shared/session";

export async function login(phone: string, password: string): Promise<{ ok: true } | { ok: false; message: string }> {
  const { data, error } = await bank.POST("/v2.0/ext/auth/login", {
    body: { phone, password, device_id: deviceId() },
  });
  if (data) {
    setToken(data.access_token);
    return { ok: true };
  }
  return { ok: false, message: errorMessage(error) };
}

export async function logout(): Promise<void> {
  await bank.POST("/oauth/2.0/revoke", {});
  setToken(undefined);
}
