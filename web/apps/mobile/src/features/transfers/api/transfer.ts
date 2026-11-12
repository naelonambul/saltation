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

export type TransferResult =
  | { ok: true; holder: string; remain: string }
  | { ok: false; message: string; retryable: boolean };

/** One transfer; the bank_tran_id is created once per attempt the user confirms. */
export async function transfer(input: TransferInput, bankTranId = newBankTranId()): Promise<TransferResult> {
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
  if (data) return { ok: true, holder: data.dps_account_holder_name, remain: data.wd_limit_remain_amt };
  return { ok: false, message: errorMessage(error), retryable: response.status === 503 };
}
