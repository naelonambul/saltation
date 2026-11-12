import { useEffect, useState } from "react";

import { won } from "../../../shared/format";
import { Notice, Screen } from "../../../shared/ui";
import { type Account, balance, listAccounts } from "../api/accounts";

export type AccountWithBalance = Account & { balance?: string };

export function HomeScreen(props: {
  onOpen: (a: AccountWithBalance) => void;
  onTransfer: (accounts: AccountWithBalance[]) => void;
  onLogout: () => void;
}) {
  const [accounts, setAccounts] = useState<AccountWithBalance[]>();
  const [message, setMessage] = useState<string>();

  useEffect(() => {
    let live = true;
    (async () => {
      const list = await listAccounts();
      if (!list.ok) return live && setMessage(list.message);
      const withBalances = await Promise.all(
        list.value.map(async (a) => {
          const b = await balance(a.fintech_use_num);
          return { ...a, balance: b.ok ? b.value : undefined };
        }),
      );
      if (live) setAccounts(withBalances);
    })();
    return () => {
      live = false;
    };
  }, []);

  return (
    <Screen title="내 계좌">
      {message && <Notice kind="error">{message}</Notice>}
      {!accounts && !message && <p className="muted">불러오는 중…</p>}
      <ul className="cards">
        {accounts?.map((a) => (
          <li key={a.fintech_use_num}>
            <button className="card" onClick={() => props.onOpen(a)} data-testid="account-card">
              <span className="muted">{a.account_alias}</span>
              <span>{a.account_num_masked}</span>
              <strong className="amount" data-testid="balance">
                {a.balance !== undefined ? won(a.balance) : "—"}
              </strong>
            </button>
          </li>
        ))}
      </ul>
      {accounts && accounts.length > 0 && (
        <button className="primary" onClick={() => props.onTransfer(accounts)}>
          이체하기
        </button>
      )}
      <button className="link" onClick={props.onLogout}>
        로그아웃
      </button>
    </Screen>
  );
}
