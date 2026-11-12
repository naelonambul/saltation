import { useState } from "react";

import { won } from "../../../shared/format";
import { newBankTranId } from "../../../shared/ids";
import { Field, Notice, Screen } from "../../../shared/ui";
import { type TransferResult, transfer } from "../api/transfer";

export type SourceAccount = { fintech_use_num: string; account_num_masked: string; account_alias: string };

export function TransferScreen(props: { accounts: SourceAccount[]; onDone: () => void; onBack: () => void }) {
  const [from, setFrom] = useState(props.accounts[0]?.fintech_use_num ?? "");
  const [toAccount, setToAccount] = useState("");
  const [toName, setToName] = useState("");
  const [amount, setAmount] = useState("");
  const [memo, setMemo] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [result, setResult] = useState<TransferResult>();
  // Reused on retry so a repeated submission is idempotent.
  const [bankTranId, setBankTranId] = useState(newBankTranId);
  const [busy, setBusy] = useState(false);

  const valid = from && /^[0-9]{1,16}$/.test(toAccount) && /^[1-9][0-9]{0,11}$/.test(amount) && toName.length > 0;

  async function send() {
    setBusy(true);
    const r = await transfer({ fintechUseNum: from, toAccount, toName, amount, memo }, bankTranId);
    setBusy(false);
    setResult(r);
    if (r.ok || !r.retryable) setBankTranId(newBankTranId());
  }

  if (result?.ok) {
    return (
      <Screen title="이체 완료" onBack={props.onDone}>
        <Notice kind="ok">
          {result.holder}님께 {won(amount)}을 보냈습니다.
        </Notice>
        <p className="muted">오늘 남은 이체한도 {won(result.remain)}</p>
        <button className="primary" onClick={props.onDone}>
          확인
        </button>
      </Screen>
    );
  }

  return (
    <Screen title="이체" onBack={props.onBack}>
      <form
        className="stack"
        onSubmit={(e) => {
          e.preventDefault();
          if (confirming) void send();
          else setConfirming(true);
        }}
      >
        <label className="field">
          <span>출금계좌</span>
          <select name="from" value={from} onChange={(e) => setFrom(e.target.value)}>
            {props.accounts.map((a) => (
              <option key={a.fintech_use_num} value={a.fintech_use_num}>
                {a.account_alias} {a.account_num_masked}
              </option>
            ))}
          </select>
        </label>
        <Field name="toAccount" label="받는 계좌번호 (살테이션은행)" inputMode="numeric" value={toAccount} onChange={setToAccount} />
        <Field name="toName" label="받는 분" value={toName} onChange={setToName} />
        <Field name="amount" label="금액(원)" inputMode="numeric" value={amount} onChange={setAmount} />
        <Field name="memo" label="내 통장 표시 (선택)" value={memo} onChange={(v) => setMemo(v.slice(0, 14))} />
        {result && !result.ok && <Notice kind="error">{result.message}</Notice>}
        {confirming ? (
          <>
            <Notice kind="ok">
              {toName}님께 {won(amount)}을 보낼까요?
            </Notice>
            <button className="primary" type="submit" disabled={busy}>
              {busy ? "보내는 중…" : "보내기"}
            </button>
            <button className="link" type="button" onClick={() => setConfirming(false)}>
              수정
            </button>
          </>
        ) : (
          <button className="primary" type="submit" disabled={!valid}>
            다음
          </button>
        )}
      </form>
    </Screen>
  );
}
