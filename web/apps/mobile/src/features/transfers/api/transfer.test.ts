import { beforeEach, describe, expect, it, vi } from "vitest";

import { bank } from "../../../shared/api";
import { abandonTransfer, confirmTransfer, networkTransferFailure, resolveTransferOutcome } from "../model/intent";
import { classifyTransferStatus, transfer, type TransferFailureOutcome } from "./transfer";

vi.mock(import("../../../shared/api"), async (importOriginal) => {
  const api = await importOriginal();
  return { ...api, bank: { ...api.bank, POST: vi.fn() } };
});

const post = vi.mocked(bank.POST);
const draft = { fintechUseNum: "123", toAccount: "456", toName: "받는 분", amount: "5000", memo: "이체" };
const ambiguousStatuses = [500, 502, 503, 504, 408, 429];
const rejectedStatuses = [400, 401, 403, 404, 409, 422];
const successData = { dps_account_holder_name: "받는 분", wd_limit_remain_amt: "45000" };
const success = { ok: true, holder: "받는 분", remain: "45000" };
const message = "이체 응답";

function failOnce(status: number) {
  post.mockResolvedValueOnce({ response: new Response(null, { status }), error: { rsp_message: message } });
}

function succeedOnce() {
  post.mockResolvedValueOnce({ response: new Response(null, { status: 200 }), data: successData });
}

beforeEach(() => post.mockReset());

describe("transfer", () => {
  it.each([0, 200, 204, 302])("classifies an unexpected HTTP %s as ambiguous", (status) => {
    expect(classifyTransferStatus(status)).toBe("ambiguous");
  });

  it.each<{ status: number; outcome: TransferFailureOutcome }>([
    ...[302, ...ambiguousStatuses].map((status) => ({ status, outcome: "ambiguous" as const })),
    ...rejectedStatuses.map((status) => ({ status, outcome: "rejected" as const })),
  ])("classifies HTTP $status as $outcome when there is no data", async ({ status, outcome }) => {
    expect(classifyTransferStatus(status)).toBe(outcome);
    failOnce(status);
    await expect(transfer(draft, "id")).resolves.toEqual({ ok: false, outcome, message, status });
  });

  it.each([
    { status: 200, data: undefined },
    { status: 204, data: undefined },
    { status: 200, data: {} as typeof successData },
  ])("preserves the intent for HTTP $status with invalid data $data", async ({ status, data }) => {
    post.mockResolvedValueOnce({ response: new Response(null, { status }), data });
    const intent = confirmTransfer(draft, () => "original");
    const result = await transfer(intent.input, intent.bankTranId);
    const message = "이체 처리 결과를 확인하지 못했습니다. 다시 보내도 중복 이체되지 않습니다.";

    expect(result).toEqual({ ok: false, outcome: "ambiguous", message, status });
    const unresolved = resolveTransferOutcome(intent, result);
    expect(unresolved.error).toEqual({ outcome: "ambiguous", message });
    expect(unresolved.intent).toBe(intent);
    expect(unresolved.intent?.bankTranId).toBe("original");
  });

  it("returns the holder and remaining limit for a successful response", async () => {
    succeedOnce();
    await expect(transfer(draft, "id")).resolves.toEqual(success);
  });
});

describe("confirmed transfer intent", () => {
  it.each([...ambiguousStatuses, "network"] as const)("retries %s with the original key and movement fields", async (failure) => {
    if (failure === "network") post.mockRejectedValueOnce(new TypeError("network"));
    else failOnce(failure);
    succeedOnce();

    const form = { ...draft };
    const createId = vi.fn(() => "original");
    const intent = confirmTransfer(form, createId);
    const result = await transfer(intent.input, intent.bankTranId).catch(() => networkTransferFailure);
    expect(result).toEqual(failure === "network" ? networkTransferFailure : { ok: false, outcome: "ambiguous", message, status: failure });
    const unresolved = resolveTransferOutcome(intent, result);
    expect(unresolved.error).toEqual({ outcome: "ambiguous", message: failure === "network" ? networkTransferFailure.message : message });
    expect(unresolved.intent).toBe(intent);
    if (!unresolved.intent) throw new Error("Retry requires an intent");

    Object.assign(form, { fintechUseNum: "789", toAccount: "987", toName: "다른 분", amount: "6000", memo: "수정" });
    const retry = await transfer(unresolved.intent.input, unresolved.intent.bankTranId);
    const movement = {
      bank_tran_id: "original",
      fintech_use_num: draft.fintechUseNum,
      tran_amt: draft.amount,
      transfer_purpose: "TR",
      wd_print_content: draft.memo,
      recv_client_name: draft.toName,
      recv_client_bank_code: "998",
      recv_client_account_num: draft.toAccount,
    };
    expect(post).toHaveBeenCalledTimes(2);
    const request = expect.objectContaining({ body: expect.objectContaining(movement) });
    expect(post).toHaveBeenNthCalledWith(1, "/v2.0/transfer/withdraw/fin_num", request);
    expect(post).toHaveBeenNthCalledWith(2, "/v2.0/transfer/withdraw/fin_num", request);
    expect(createId).toHaveBeenCalledTimes(1);

    const completed = resolveTransferOutcome(unresolved.intent, retry, { priorAmbiguous: true });
    expect(completed).toEqual({
      intent: null,
      completed: { holder: success.holder, remain: success.remain, input: draft },
      error: null,
    });
    expect(completed.completed?.input).toBe(intent.input);
  });

  it.each([401, 403, 409])("keeps the original intent when a lost response is followed by HTTP %s", async (status) => {
    post.mockRejectedValueOnce(new TypeError("lost response"));
    failOnce(status);
    const intent = confirmTransfer(draft, () => "original");
    const first = await transfer(intent.input, intent.bankTranId).catch(() => networkTransferFailure);
    const unresolved = resolveTransferOutcome(intent, first);
    if (!unresolved.intent) throw new Error("Retry requires an intent");

    const retry = await transfer(unresolved.intent.input, unresolved.intent.bankTranId);
    expect(retry).toEqual({ ok: false, outcome: "rejected", message, status });
    const retried = resolveTransferOutcome(unresolved.intent, retry, { priorAmbiguous: unresolved.error?.outcome === "ambiguous" });
    expect(retried.error).toEqual({ outcome: "ambiguous", message });
    expect(retried.intent).toBe(intent);
    expect(retried.intent?.bankTranId).toBe("original");
  });

  it.each(rejectedStatuses)("clears the intent after first-attempt HTTP %s so the next confirmation gets a new key", async (status) => {
    const createId = vi.fn<() => string>().mockReturnValueOnce("original").mockReturnValueOnce("fresh");
    const intent = confirmTransfer(draft, createId);
    failOnce(status);
    const result = await transfer(intent.input, intent.bankTranId);

    expect(resolveTransferOutcome(intent, result)).toEqual({
      intent: null,
      completed: null,
      error: { outcome: "rejected", message },
    });
    const next = confirmTransfer({ ...draft, amount: "6000" }, createId);
    expect(next.bankTranId).toBe("fresh");
    expect(next.input.amount).toBe("6000");
    expect(createId).toHaveBeenCalledTimes(2);
  });

  it("explicitly abandons the unresolved intent and its error", () => {
    const unresolved = resolveTransferOutcome(confirmTransfer(draft, () => "original"), networkTransferFailure);
    expect(unresolved.intent?.bankTranId).toBe("original");
    expect(unresolved.error?.outcome).toBe("ambiguous");
    expect(abandonTransfer()).toEqual({ intent: null, completed: null, error: null });
  });

  it("freezes the intent and copies the draft before it can change", () => {
    const form = { ...draft };
    const intent = confirmTransfer(form, () => "original");
    form.amount = "6000";

    expect(intent.input).not.toBe(form);
    expect(intent.input).toEqual(draft);
    expect(Object.isFrozen(intent)).toBe(true);
    expect(Object.isFrozen(intent.input)).toBe(true);
  });
});
