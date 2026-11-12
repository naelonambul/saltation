// The bank client used by every feature, and the common error message.
import { createBankClient } from "@saltation/api-client";

import { getToken } from "./session";

export const API_BASE: string = import.meta.env.VITE_API_BASE ?? "http://localhost:8080";

export const bank = createBankClient(API_BASE, getToken);

/** The Korean message from an error envelope, or a generic one. */
export function errorMessage(error: unknown): string {
  if (error && typeof error === "object" && "rsp_message" in error) {
    const m = (error as { rsp_message?: unknown }).rsp_message;
    if (typeof m === "string" && m.length > 0) return m;
  }
  return "요청을 처리하지 못했습니다. 잠시 후 다시 시도해 주세요.";
}
