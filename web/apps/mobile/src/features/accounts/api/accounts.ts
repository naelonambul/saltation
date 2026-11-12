import type { Schemas } from "@saltation/api-client";

import { bank, errorMessage } from "../../../shared/api";
import { newBankTranId, tranDtime, ymd } from "../../../shared/ids";

export type Account = Schemas["AccountItem"];
export type Transaction = Schemas["TransactionItem"];
export type Result<T> = { ok: true; value: T } | { ok: false; message: string };

export async function listAccounts(): Promise<Result<Account[]>> {
  const { data, error } = await bank.GET("/v2.0/account/list");
  return data ? { ok: true, value: data.res_list } : { ok: false, message: errorMessage(error) };
}

export async function balance(fintechUseNum: string): Promise<Result<string>> {
  const { data, error } = await bank.GET("/v2.0/account/balance/fin_num", {
    params: { query: { bank_tran_id: newBankTranId(), fintech_use_num: fintechUseNum, tran_dtime: tranDtime() } },
  });
  return data ? { ok: true, value: data.balance_amt } : { ok: false, message: errorMessage(error) };
}

export type Page = { rows: Transaction[]; next?: string; balance: string };

/** One page (25 rows) of the last 90 days, newest first. */
export async function history(fintechUseNum: string, cursor?: string): Promise<Result<Page>> {
  const to = new Date();
  const from = new Date(to.getTime() - 90 * 24 * 3600 * 1000);
  const { data, error } = await bank.GET("/v2.0/account/transaction_list/fin_num", {
    params: {
      query: {
        bank_tran_id: newBankTranId(),
        fintech_use_num: fintechUseNum,
        inquiry_type: "A",
        inquiry_base: "D",
        from_date: ymd(from),
        to_date: ymd(to),
        sort_order: "D",
        tran_dtime: tranDtime(),
        ...(cursor ? { befor_inquiry_trace_info: cursor } : {}),
      },
    },
  });
  if (!data) return { ok: false, message: errorMessage(error) };
  return {
    ok: true,
    value: {
      rows: data.res_list,
      next: data.next_page_yn === "Y" ? data.befor_inquiry_trace_info : undefined,
      balance: data.balance_amt,
    },
  };
}
