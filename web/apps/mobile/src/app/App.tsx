// Screen flow: login -> home -> account history or transfer.
// The app composes features; features never import each other.
import { useState } from "react";

import { AccountScreen, type AccountWithBalance, HomeScreen } from "../features/accounts";
import { LoginScreen, logout } from "../features/auth";
import { TransferScreen } from "../features/transfers";
import { getToken } from "../shared/session";

type View =
  | { name: "login" }
  | { name: "home" }
  | { name: "account"; account: AccountWithBalance }
  | { name: "transfer"; accounts: AccountWithBalance[] };

export function App() {
  const [view, setView] = useState<View>(getToken() ? { name: "home" } : { name: "login" });

  switch (view.name) {
    case "login":
      return <LoginScreen onLoggedIn={() => setView({ name: "home" })} />;
    case "home":
      return (
        <HomeScreen
          onOpen={(account) => setView({ name: "account", account })}
          onTransfer={(accounts) => setView({ name: "transfer", accounts })}
          onLogout={async () => {
            await logout();
            setView({ name: "login" });
          }}
        />
      );
    case "account":
      return (
        <AccountScreen
          fintechUseNum={view.account.fintech_use_num}
          title={view.account.account_alias}
          onBack={() => setView({ name: "home" })}
        />
      );
    case "transfer":
      return (
        <TransferScreen
          accounts={view.accounts}
          onDone={() => setView({ name: "home" })}
          onBack={() => setView({ name: "home" })}
        />
      );
  }
}
