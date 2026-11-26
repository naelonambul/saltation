# Plan — mobile-transfer-immutable-intent

## Context

GPT-6 Astra Pro audit finding `F-02`: `web/apps/mobile/src/features/transfers/api/transfer.ts` currently marks only HTTP `503` as `retryable`, and `TransferScreen.tsx` rotates `bankTranId` whenever `r.ok || !r.retryable`. An ambiguous server error (`500`, `502`, `504`, `408`, `429`) after the bank commits therefore discards the original idempotency key, so a user retry can execute a duplicate transfer. In addition, form fields remain editable during confirmation/ambiguous states, risking mutated movement fields under the same key.

## Implementation steps

1. **Classify transfer outcomes cleanly (`web/apps/mobile/src/features/transfers/api/transfer.ts`)**
   - Distinguish three explicit outcomes in `TransferResult`:
     - `{ ok: true; holder: string; remain: string }`
     - `{ ok: false; outcome: "ambiguous"; message: string }` for HTTP `>= 500`, `408`, `429` (where commit state may be unknown; caller must keep `bankTranId` and canonical movement fields)
     - `{ ok: false; outcome: "rejected"; message: string }` for definitive 4xx rejections (`400`, `401`, `403`, `404`, `409`, `422`, etc.)
   - Extract a small pure helper `classifyTransferStatus(status: number): "ambiguous" | "rejected"` so outcome classification is directly unit-tested.

2. **Bind confirmed transfers to an immutable intent (`web/apps/mobile/src/features/transfers/components/TransferScreen.tsx`, `web/apps/mobile/src/shared/ui.tsx`)**
   - Add optional `disabled?: boolean` prop to `Field` in `web/apps/mobile/src/shared/ui.tsx`.
   - Extract a pure state reducer/helper for transfer intent lifecycle (`editing` → `confirmedIntent: { bankTranId, input }` → `ambiguous` / `done` / `rejected`):
     - Entering confirmation snapshots `{ bankTranId: newBankTranId(), input }` as an immutable `confirmedIntent`.
     - While `confirmedIntent` is active (confirming, sending, or holding an `ambiguous` error), lock all movement inputs (`select` and `Field`s) so the user cannot mutate fields under an existing key.
     - On `ambiguous` HTTP error or thrown network error: keep `confirmedIntent` unchanged so retrying sends the exact same `bankTranId` and exact same `input` fields. Show a clear Korean retry notice and require an explicit "새 이체 작성" (abandon) action to discard an unresolved intent.
     - On definitive `rejected`: clear `confirmedIntent` and unlock editing so the next confirmation allocates a fresh `bankTranId`.
     - On `ok: true`: show completion using the confirmed intent's immutable amount and clear `confirmedIntent`.

3. **Add unit tests (`web/apps/mobile/src/features/transfers/api/transfer.test.ts`)**
   - Test `transfer()` and `classifyTransferStatus()` across `200`, `500`, `502`, `503`, `504`, `408`, `429`, `400`, `409`, `422`.
   - Test the transfer-intent state machine across:
     - `500`/`502`/`503`/`504`/network-throw preserving the exact `bankTranId` and canonical `input` on retry;
     - definitive `400`/`422` clearing the intent and generating a fresh `bankTranId` on next confirm;
     - explicit abandon clearing an unresolved intent;
     - immutability of `confirmedIntent.input` while locked.

## Verification

- Run `pnpm --dir web --filter @saltation/mobile test` (Vitest).
- Run `pnpm --dir web --filter @saltation/mobile typecheck` and `pnpm --dir web --filter @saltation/mobile build`.
- Run `pnpm --dir web lint` (Biome / ESLint).
- Run `python3 scripts/repo.py status --change mobile-transfer-immutable-intent --base stage6-expected-decline-resume`.
- Commit locally on branch `mobile-transfer-immutable-intent` (no push while OCI 365-day backfill is active).
