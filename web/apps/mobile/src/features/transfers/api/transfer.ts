import { bank, errorMessage } from "../../../shared/api";
import { newBankTranId, tranDtime } from "../../../shared/ids";

/** Saltation's bank_code_std (contracts/codes/institutions.json). */
export const SALTATION_BANK = "998";

export type TransferInput = {
  fintechUseNum: string;
  toAccount: string;
  toName: string;
  amount: string;
  memo: string;
};

export type TransferFailureOutcome = "ambiguous" | "rejected";

export type TransferResult =
  | { ok: true; holder: string; remain: string }
  | { ok: false; outcome: TransferFailureOutcome; message: string; status?: number };

export function classifyTransferStatus(status: number): TransferFailureOutcome {
  return status >= 400 && status < 500 && status !== 408 && status !== 429 ? "rejected" : "ambiguous";
}

/** One transfer; the bank_tran_id is created once per attempt the user confirms. */
export async function transfer(input: Readonly<TransferInput>, bankTranId = newBankTranId()): Promise<TransferResult> {
  const { data, error, response } = await bank.POST("/v2.0/transfer/withdraw/fin_num", {
    body: {
      bank_tran_id: bankTranId,
      fintech_use_num: input.fintechUseNum,
      tran_amt: input.amount,
      tran_dtime: tranDtime(),
      transfer_purpose: "TR",
      wd_print_content: input.memo,
      recv_client_name: input.toName,
      recv_client_bank_code: SALTATION_BANK,
      recv_client_account_num: input.toAccount,
    },
  });
  if (response.ok) {
    if (typeof data?.dps_account_holder_name === "string" && typeof data?.wd_limit_remain_amt === "string") {
      return { ok: true, holder: data.dps_account_holder_name, remain: data.wd_limit_remain_amt };
    }
    return {
      ok: false,
      outcome: "ambiguous",
      message: "이체 처리 결과를 확인하지 못했습니다. 다시 보내도 중복 이체되지 않습니다.",
      status: response.status,
    };
  }
  return { ok: false, outcome: classifyTransferStatus(response.status), message: errorMessage(error), status: response.status };
}
