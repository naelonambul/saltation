import { useEffect, useState } from "react";

import { dateTime, won } from "../../../shared/format";
import { Notice, Screen } from "../../../shared/ui";
import { type Page, type Result, type Transaction, history } from "../api/accounts";

export function AccountScreen(props: { fintechUseNum: string; title: string; onBack: () => void }) {
  const [rows, setRows] = useState<Transaction[]>([]);
  const [next, setNext] = useState<string>();
  const [balanceAmt, setBalanceAmt] = useState<string>();
  const [message, setMessage] = useState<string>();

  function apply(page: Result<Page>, append: boolean) {
    if (!page.ok) return setMessage(page.message);
    setRows((prev) => (append ? [...prev, ...page.value.rows] : page.value.rows));
    setNext(page.value.next);
    setBalanceAmt(page.value.balance);
  }

  useEffect(() => {
    let live = true;
    history(props.fintechUseNum).then((page) => {
      if (live) apply(page, false);
    });
    return () => {
      live = false;
    };
  }, [props.fintechUseNum]);

  async function more(cursor: string) {
    apply(await history(props.fintechUseNum, cursor), true);
  }

  return (
    <Screen title={props.title} onBack={props.onBack}>
      <p className="balance">{balanceAmt !== undefined ? won(balanceAmt) : "…"}</p>
      {message && <Notice kind="error">{message}</Notice>}
      <ul className="rows" data-testid="history">
        {rows.map((r, i) => (
          <li key={`${r.tran_date}${r.tran_time}${i}`}>
            <div>
              <span>{r.print_content || r.tran_type}</span>
              <span className="muted">{dateTime(r.tran_date, r.tran_time)}</span>
            </div>
            <div className="right">
              <strong className={r.inout_type === "입금" ? "in" : "out"}>
                {r.inout_type === "입금" ? "+" : "−"}
                {won(r.tran_amt)}
              </strong>
              <span className="muted">{won(r.after_balance_amt)}</span>
            </div>
          </li>
        ))}
      </ul>
      {rows.length === 0 && !message && <p className="muted">거래내역이 없습니다.</p>}
      {next && (
        <button className="link" onClick={() => void more(next)}>
          더 보기
        </button>
      )}
    </Screen>
  );
}
