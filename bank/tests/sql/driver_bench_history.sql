-- Test fixture: preserve the funded opening leg, then append 90 zero-net days.
DECLARE
  v_day DATE := :business_date;
  v_at TIMESTAMP WITH TIME ZONE;
  v_request VARCHAR2(26) := :request_id;
  v_amount NUMBER;
BEGIN
  INSERT INTO transaction_request
    (request_id, request_type, channel, listener, idem_scope, idem_key,
     request_hash, status, rsp_code, occurred_at)
  VALUES (v_request, 'TEST', 'SYSTEM', 'GENERATOR', 'driver_bench', v_request,
          'x', 'DONE', 'A0000', FROM_TZ(CAST(v_day AS TIMESTAMP), '+09:00'));
  FOR a IN (SELECT account_id, balance, next_sequence FROM account ORDER BY account_id) LOOP
    -- The generator's funded opening precedes this historical window.
    UPDATE account_transaction
       SET business_date = v_day - 91,
           occurred_at = FROM_TZ(CAST(v_day - 91 AS TIMESTAMP), '+09:00')
     WHERE account_id = a.account_id;
    UPDATE account
       SET opened_at = FROM_TZ(CAST(v_day - 91 AS TIMESTAMP), '+09:00'),
           next_sequence = a.next_sequence + 90
     WHERE account_id = a.account_id;
    FOR d IN 1..90 LOOP
      v_amount := CASE WHEN MOD(d, 2) = 1 THEN 1000 ELSE -1000 END;
      v_at := FROM_TZ(CAST(v_day - 91 + d AS TIMESTAMP), '+09:00');
      INSERT INTO account_transaction
        (account_id, account_sequence, amount, balance_after, inout_type,
         tran_type, print_content, channel, request_id, business_date, occurred_at)
      VALUES (a.account_id, a.next_sequence + d - 1, v_amount,
              a.balance + CASE WHEN MOD(d, 2) = 1 THEN 1000 ELSE 0 END,
              CASE WHEN v_amount > 0 THEN '입금' ELSE '출금' END,
              '대체', '벤치마크', 'SYSTEM', v_request, v_day - 91 + d, v_at);
    END LOOP;
  END LOOP;
END;
