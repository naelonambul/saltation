import { useState } from "react";

import { Field, Notice, Screen } from "../../../shared/ui";
import { login } from "../api/login";

export function LoginScreen({ onLoggedIn }: { onLoggedIn: () => void }) {
  const [phone, setPhone] = useState("");
  const [password, setPassword] = useState("");
  const [message, setMessage] = useState<string>();
  const [busy, setBusy] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setMessage(undefined);
    const result = await login(phone, password);
    setBusy(false);
    if (result.ok) onLoggedIn();
    else setMessage(result.message);
  }

  return (
    <Screen title="살테이션은행">
      <form className="stack" onSubmit={submit}>
        <p className="lead">전화번호와 비밀번호로 로그인하세요.</p>
        <Field name="phone" label="전화번호" inputMode="tel" value={phone} onChange={setPhone} />
        <Field name="password" label="비밀번호" type="password" value={password} onChange={setPassword} />
        {message && <Notice kind="error">{message}</Notice>}
        <button className="primary" type="submit" disabled={busy || !phone || !password}>
          {busy ? "확인 중…" : "로그인"}
        </button>
      </form>
    </Screen>
  );
}
