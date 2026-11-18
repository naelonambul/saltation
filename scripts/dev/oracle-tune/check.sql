SET SERVEROUTPUT ON
DECLARE
  bad NUMBER;
  PROCEDURE check_count(label VARCHAR2, n NUMBER) IS
  BEGIN
    DBMS_OUTPUT.PUT_LINE(label||': '||n);
    IF n<>0 THEN RAISE_APPLICATION_ERROR(-20002,'tune consistency failed: '||label); END IF;
  END;
BEGIN
  SELECT COUNT(*) INTO bad FROM account a LEFT JOIN
    (SELECT account_id,SUM(amount) balance,COUNT(*) n,MIN(account_sequence) lo,MAX(account_sequence) hi,
      MAX(balance_after) KEEP(DENSE_RANK LAST ORDER BY account_sequence) last_balance
     FROM account_transaction GROUP BY account_id) r ON r.account_id=a.account_id
    WHERE a.balance<>NVL(r.balance,0) OR a.balance<>r.last_balance OR a.next_sequence<>NVL(r.hi,0)+1 OR r.lo<>1 OR r.n<>r.hi;
  check_count('balance / next_sequence / contiguous sequences',bad);
  SELECT COUNT(*) INTO bad FROM
    (SELECT balance_after,SUM(amount) OVER(PARTITION BY account_id ORDER BY account_sequence) expected FROM account_transaction)
    WHERE balance_after<>expected;
  check_count('running balances',bad);
  SELECT COUNT(*) INTO bad FROM transfer t LEFT JOIN
    (SELECT transfer_id,COUNT(*) n,COUNT(DISTINCT account_id) accounts,SUM(amount) net,MIN(ABS(amount)) lo,MAX(ABS(amount)) hi FROM account_transaction GROUP BY transfer_id) r
    ON t.transfer_id=r.transfer_id WHERE r.n IS NULL OR r.lo<>t.amount OR r.hi<>t.amount OR
    (t.kind='transfer' AND (r.n<>2 OR r.accounts<>2 OR r.net<>0)) OR (t.kind='inbound' AND (r.n<>1 OR r.net<>t.amount));
  check_count('transfer shapes',bad);
  SELECT COUNT(*) INTO bad FROM account_transaction r LEFT JOIN account a ON a.account_id=r.account_id
    LEFT JOIN transfer t ON t.transfer_id=r.transfer_id LEFT JOIN transaction_request q ON q.request_id=r.request_id
    WHERE a.account_id IS NULL OR t.transfer_id IS NULL OR (r.request_id IS NOT NULL AND q.request_id IS NULL);
  check_count('ledger orphans',bad);
  SELECT COUNT(*) INTO bad FROM daily_account_total d FULL OUTER JOIN
    (SELECT account_id,business_date,SUM(-amount) total FROM account_transaction WHERE amount<0 AND tran_type='대체' GROUP BY account_id,business_date) r
    ON r.account_id=d.account_id AND r.business_date=d.business_date
    WHERE NVL(d.transfer_out,0)<>NVL(r.total,0) OR NVL(d.cash_in,0)<>0 OR NVL(d.cash_out,0)<>0;
  check_count('daily totals',bad);
  SELECT COUNT(*) INTO bad FROM daily_close c LEFT JOIN
    (SELECT account_id,business_date,SUM(amount) net FROM account_transaction GROUP BY account_id,business_date) r
    ON r.account_id=c.account_id AND r.business_date=c.business_date
    WHERE c.closing_balance<>c.opening_balance+NVL(r.net,0) OR c.verified<>'Y';
  check_count('daily close deltas',bad);
  SELECT COUNT(*) INTO bad FROM daily_close c JOIN daily_close p ON p.account_id=c.account_id AND p.business_date=c.business_date-1
    WHERE c.opening_balance<>p.closing_balance;
  check_count('daily close continuity',bad);
  SELECT COUNT(*) INTO bad FROM account a CROSS JOIN tune_seed s WHERE
    (SELECT COUNT(*) FROM daily_close c WHERE c.account_id=a.account_id)<>s.last_day-s.first_day;
  check_count('daily close coverage',bad);
  SELECT COUNT(*) INTO bad FROM daily_close c JOIN tune_seed s ON 1=1 WHERE
    (c.business_date=s.first_day AND c.opening_balance<>0) OR
    (c.business_date=s.last_day-1 AND c.closing_balance<>(SELECT NVL(SUM(r.amount),0) FROM account_transaction r WHERE r.account_id=c.account_id AND r.business_date<=c.business_date));
  check_count('daily close boundary balances',bad);
  SELECT COUNT(*) INTO bad FROM dba_constraints WHERE owner='SALTATION_TUNE' AND constraint_type='R' AND (status<>'ENABLED' OR validated<>'VALIDATED');
  check_count('foreign keys enabled and validated (all tables)',bad);
  DBMS_OUTPUT.PUT_LINE('tune consistency: PASS');
END;
/
