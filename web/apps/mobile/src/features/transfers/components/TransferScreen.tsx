import { useRef, useState } from "react";

import { won } from "../../../shared/format";
import { newBankTranId } from "../../../shared/ids";
import { Field, Notice, Screen } from "../../../shared/ui";
import { type TransferResult, transfer } from "../api/transfer";
import { abandonTransfer, confirmTransfer, networkTransferFailure, resolveTransferOutcome } from "../model/intent";

export type SourceAccount = { fintech_use_num: string; account_num_masked: string; account_alias: string };

export function TransferScreen(props: { accounts: SourceAccount[]; onDone: () => void; onBack: () => void }) {
  const [from, setFrom] = useState(props.accounts[0]?.fintech_use_num ?? "");
  const [toAccount, setToAccount] = useState("");
  const [toName, setToName] = useState("");
  const [amount, setAmount] = useState("");
  const [memo, setMemo] = useState("");
  const [{ intent: confirmedIntent, completed, error }, setTransferState] = useState(abandonTransfer);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);

  const valid = from && /^[0-9]{1,16}$/.test(toAccount) && /^[1-9][0-9]{0,11}$/.test(amount) && toName.length > 0;
  const locked = confirmedIntent !== null || busy;
  const ambiguous = error?.outcome === "ambiguous";

  async function send() {
    if (!confirmedIntent || inFlight.current) return;

    inFlight.current = true;
    setBusy(true);
    let result: TransferResult;
    try {
      result = await transfer(confirmedIntent.input, confirmedIntent.bankTranId);
    } catch {
      result = networkTransferFailure;
    }
    setTransferState(resolveTransferOutcome(confirmedIntent, result));
    setBusy(false);
    inFlight.current = false;
  }

  function editTransfer() {
    if (
      ambiguous &&
      !window.confirm("이전 이체가 완료되었을 수 있습니다. 새 이체를 보내기 전에 잔액과 거래내역을 확인해 주세요. 새 이체를 작성할까요?")
    ) return;

    setTransferState(abandonTransfer());
  }

  if (completed) {
    return (
      <Screen title="이체 완료" onBack={props.onDone}>
        <Notice kind="ok">
          {completed.holder}님께 {won(completed.input.amount)}을 보냈습니다.
        </Notice>
        <p className="muted">오늘 남은 이체한도 {won(completed.remain)}</p>
        <button className="primary" onClick={props.onDone}>
          확인
        </button>
      </Screen>
    );
  }

  return (
    <Screen title="이체" onBack={busy || ambiguous ? undefined : props.onBack}>
      <form
        className="stack"
        onSubmit={(e) => {
          e.preventDefault();
          if (busy) return;
          if (confirmedIntent) void send();
          else if (valid) {
            const intent = confirmTransfer({ fintechUseNum: from, toAccount, toName, amount, memo }, newBankTranId);
            setTransferState({ intent, completed: null, error: null });
          }
        }}
      >
        <label className="field">
          <span>출금계좌</span>
          <select name="from" value={from} disabled={locked} onChange={(e) => setFrom(e.target.value)}>
            {props.accounts.map((a) => (
              <option key={a.fintech_use_num} value={a.fintech_use_num}>
                {a.account_alias} {a.account_num_masked}
              </option>
            ))}
          </select>
        </label>
        <Field
          name="toAccount"
          label="받는 계좌번호 (살테이션은행)"
          inputMode="numeric"
          value={toAccount}
          disabled={locked}
          onChange={setToAccount}
        />
        <Field name="toName" label="받는 분" value={toName} disabled={locked} onChange={setToName} />
        <Field name="amount" label="금액(원)" inputMode="numeric" value={amount} disabled={locked} onChange={setAmount} />
        <Field name="memo" label="내 통장 표시 (선택)" value={memo} disabled={locked} onChange={(v) => setMemo(v.slice(0, 14))} />
        {error && <Notice kind="error">{error.message}</Notice>}
        {confirmedIntent ? (
          <>
            <Notice kind="ok">
              {confirmedIntent.input.toName}님께 {won(confirmedIntent.input.amount)}을 보낼까요?
            </Notice>
            <button className="primary" type="submit" disabled={busy}>
              {ambiguous ? "다시 시도" : "보내기"}
            </button>
            <button className="link" type="button" disabled={busy} onClick={editTransfer}>
              {ambiguous ? "새 이체 작성" : "수정"}
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
