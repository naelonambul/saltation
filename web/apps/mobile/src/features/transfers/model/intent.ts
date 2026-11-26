import type { TransferInput, TransferResult } from "../api/transfer";

export type ConfirmedTransferIntent = {
  readonly bankTranId: string;
  readonly input: Readonly<TransferInput>;
};

type CompletedTransfer = { holder: string; remain: string; input: ConfirmedTransferIntent["input"] };
type TransferError<Outcome> = { outcome: Outcome; message: string };

export type TransferState =
  | { intent: null; completed: null; error: TransferError<"rejected"> | null }
  | { intent: ConfirmedTransferIntent; completed: null; error: TransferError<"ambiguous"> | null }
  | { intent: null; completed: CompletedTransfer; error: null };

export const networkTransferFailure = {
  ok: false,
  outcome: "ambiguous",
  message: "서버에 연결하지 못했습니다. 다시 보내도 중복 이체되지 않습니다.",
} as const satisfies TransferResult;

export function confirmTransfer(input: TransferInput, createId: () => string): ConfirmedTransferIntent {
  return Object.freeze({ bankTranId: createId(), input: Object.freeze({ ...input }) });
}

export function resolveTransferOutcome(
  intent: ConfirmedTransferIntent,
  result: TransferResult,
  { priorAmbiguous = false }: { priorAmbiguous?: boolean } = {},
): TransferState {
  if (result.ok) {
    return { intent: null, completed: { holder: result.holder, remain: result.remain, input: intent.input }, error: null };
  }
  const { outcome, message, status } = result;
  if (outcome === "ambiguous" || (priorAmbiguous && (status === 401 || status === 403 || status === 409))) {
    return { intent, completed: null, error: { outcome: "ambiguous", message } };
  }
  return { intent: null, completed: null, error: { outcome, message } };
}

export function abandonTransfer(): TransferState {
  return { intent: null, completed: null, error: null };
}
