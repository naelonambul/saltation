// Typed client for the bank's public interface, generated from
// contracts/openapi/public.yaml (src/schema.ts is generated, not committed).
import createClient from "openapi-fetch";

import type { components, paths } from "./schema";

export type Schemas = components["schemas"];
export type BankClient = ReturnType<typeof createBankClient>;

export function createBankClient(baseUrl: string, token?: () => string | undefined) {
  const client = createClient<paths>({ baseUrl });
  client.use({
    onRequest({ request }) {
      const t = token?.();
      if (t) request.headers.set("Authorization", `Bearer ${t}`);
      return request;
    },
  });
  return client;
}
