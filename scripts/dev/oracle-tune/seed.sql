-- Run as SYSDBA with CURRENT_SCHEMA=SALTATION_TUNE. No migrations are copied here.
SET SERVEROUTPUT ON
DECLARE
  n PLS_INTEGER := &customers;
  years PLS_INTEGER := &years;
  max_bytes NUMBER := &max_gb * POWER(1024,3);
  today DATE;
  first_day DATE;
  d DATE;
  at_time TIMESTAMP WITH TIME ZONE;
  base_id NUMBER := n;
  moves NUMBER;
  used_bytes NUMBER;
  hot_pairs PLS_INTEGER := GREATEST(1, FLOOR(n/200));
  hot_per_day PLS_INTEGER;
  PROCEDURE space_check IS
  BEGIN
    SELECT NVL(SUM(bytes),0) INTO used_bytes FROM dba_segments WHERE tablespace_name='SALTATION_DATA';
    -- Reserve the next day's worst-case rows and index growth before writing it.
    IF used_bytes + (hot_pairs*hot_per_day + n)*8192 > max_bytes THEN
      RAISE_APPLICATION_ERROR(-20001,'tune-seed: SALTATION_DATA would exceed TUNE_MAX_GB');
    END IF;
  END;
BEGIN
  SELECT TRUNC(CAST(business_time AT TIME ZONE '+09:00' AS DATE)) INTO today FROM sim_clock;
  first_day := ADD_MONTHS(today,-12*years);
  hot_per_day := 2*CEIL(50000/(today-first_day+1)/2);
  space_check;
  INSERT INTO tune_seed VALUES(n,years,&max_gb,first_day,today,hot_pairs,hot_per_day);
  INSERT /*+ APPEND */ INTO customer(customer_id,customer_no,ci_hash,name,phone,customer_type,created_at)
  SELECT LEVEL,'C'||LPAD(LEVEL,10,'0'),LOWER(STANDARD_HASH('tune-ci-'||LEVEL,'SHA256')),
    'Synthetic '||LEVEL,'010'||LPAD(LEVEL,8,'0'),'INDIVIDUAL',FROM_TZ(CAST(first_day AS TIMESTAMP),'+09:00')
  FROM dual CONNECT BY LEVEL<=n;
  COMMIT;
  INSERT /*+ APPEND */ INTO account(account_id,account_no,customer_id,terms_id,subtype,status,balance,next_sequence,opened_at)
  SELECT customer_id,'9600'||LPAD(customer_id,8,'0'),customer_id,1,'DEMAND','ACTIVE',100000000,2,created_at FROM customer;
  COMMIT;
  INSERT INTO account_status_history(account_id,to_status,reason,occurred_at)
  SELECT account_id,'ACTIVE','seed',opened_at FROM account;
  INSERT INTO fintech_account SELECT '998'||LPAD(account_id,21,'0'),account_id,LPAD(customer_id,10,'0') FROM account;
  INSERT INTO counterparty(counterparty_id,counterparty_type,institution_code,account_no,name,created_at)
  SELECT LEVEL,'EMPLOYER','901','9700'||LPAD(LEVEL,8,'0'),'Synthetic employer '||LEVEL,
    FROM_TZ(CAST(first_day AS TIMESTAMP),'+09:00') FROM dual CONNECT BY LEVEL<=100;
  INSERT INTO payee(customer_id,bank_code,account_no,holder_name,registered_at,channel)
  SELECT a.customer_id,'998',b.account_no,'Synthetic '||b.customer_id,a.opened_at,'MOBILE'
  FROM account a JOIN account b ON b.account_id=CASE WHEN MOD(a.account_id,2)=1 THEN a.account_id+1 ELSE a.account_id-1 END;
  INSERT /*+ APPEND */ INTO transfer(transfer_id,kind,amount,occurred_at)
  SELECT account_id,'inbound',100000000,opened_at FROM account;
  COMMIT;
  INSERT /*+ APPEND */ INTO account_transaction(account_id,account_sequence,amount,balance_after,inout_type,tran_type,print_content,channel,transfer_id,business_date,occurred_at)
  SELECT account_id,1,100000000,100000000,'입금','대체','개설입금','SYSTEM',account_id,first_day,opened_at FROM account;
  COMMIT;
  d := first_day;
  WHILE d<=today LOOP
    space_check;
    at_time := FROM_TZ(CAST(d AS TIMESTAMP),'+09:00') + INTERVAL '12' HOUR;
    -- One staging row per movement, two ledger legs. Cold pairs alternate direction daily.
    DELETE FROM tune_moves;
    INSERT /*+ APPEND */ INTO tune_moves
    SELECT base_id+ROW_NUMBER() OVER(ORDER BY p,k),2*p-1,2*p,
      CASE WHEN MOD(k+TRUNC(d-first_day),2)=1 THEN -100 ELSE 100 END,k
    FROM (SELECT LEVEL p FROM dual CONNECT BY LEVEL<=FLOOR(n/2))
    CROSS JOIN (SELECT LEVEL k FROM dual CONNECT BY LEVEL<=hot_per_day)
    WHERE k<=CASE WHEN p<=hot_pairs THEN hot_per_day WHEN MOD(TRUNC(d-first_day),2)=0 THEN 1 ELSE 0 END;
    COMMIT;
    SELECT COUNT(*) INTO moves FROM tune_moves;
    INSERT /*+ APPEND */ INTO transaction_request(request_id,request_type,channel,listener,idem_scope,idem_key,request_hash,bank_tran_id,status,rsp_code,bank_rsp_code,response_json,occurred_at)
    SELECT LPAD(tid,26,'0'),'TRANSFER','MOBILE','PUBLIC','C:'||CASE WHEN amount<0 THEN a1 ELSE a2 END,
      'T000000002U'||LPAD(tid,9,'0'),LOWER(STANDARD_HASH('tune-request-'||tid,'SHA256')),
      'T000000002U'||LPAD(tid,9,'0'),'DONE','A0000','000',
      JSON_OBJECT('rsp_code' VALUE 'A0000','rsp_message' VALUE 'Synthetic success',
        'bank_tran_id' VALUE 'T000000002U'||LPAD(tid,9,'0'),'bank_tran_date' VALUE TO_CHAR(d,'YYYYMMDD'),
        'bank_code_tran' VALUE '998','bank_rsp_code' VALUE '000','bank_rsp_message' VALUE '',
        'bank_name' VALUE 'Synthetic bank','fintech_use_num' VALUE '998'||LPAD(CASE WHEN amount<0 THEN a1 ELSE a2 END,21,'0'),
        'tran_amt' VALUE '100','account_num_masked' VALUE '9600****0000','print_content' VALUE 'Synthetic transfer',
        'wd_limit_remain_amt' VALUE '49990000','dps_bank_code_std' VALUE '998','dps_bank_name' VALUE 'Synthetic bank',
        'dps_account_num_masked' VALUE '9600****0000','dps_print_content' VALUE 'Synthetic transfer',
        'dps_account_holder_name' VALUE 'Synthetic recipient'),at_time+NUMTODSINTERVAL(k/1000,'SECOND') FROM tune_moves;
    COMMIT;
    INSERT /*+ APPEND */ INTO transfer(transfer_id,kind,request_id,bank_tran_id,amount,occurred_at,payee_registered)
    SELECT tid,'transfer',LPAD(tid,26,'0'),'T000000002U'||LPAD(tid,9,'0'),100,at_time+NUMTODSINTERVAL(k/1000,'SECOND'),'Y' FROM tune_moves;
    COMMIT;
    INSERT /*+ APPEND */ INTO account_transaction(account_id,account_sequence,amount,balance_after,inout_type,tran_type,print_content,counterparty_bank_code,counterparty_account_no,counterparty_name,channel,request_id,transfer_id,business_date,occurred_at)
    SELECT a.account_id,a.next_sequence+ROW_NUMBER() OVER(PARTITION BY a.account_id ORDER BY m.k)-1,
      m.amount,a.balance+SUM(m.amount) OVER(PARTITION BY a.account_id ORDER BY m.k),
      CASE WHEN m.amount>0 THEN '입금' ELSE '출금' END,'대체','Synthetic transfer','998',b.account_no,'Synthetic '||b.customer_id,'MOBILE',LPAD(m.tid,26,'0'),m.tid,d,at_time+NUMTODSINTERVAL(m.k/1000,'SECOND')
    FROM (SELECT tid,a1 aid,a2 other,amount,k FROM tune_moves UNION ALL SELECT tid,a2,a1,-amount,k FROM tune_moves) m
    JOIN account a ON a.account_id=m.aid JOIN account b ON b.account_id=m.other;
    COMMIT;
    INSERT INTO daily_account_total(account_id,business_date,transfer_out)
    SELECT aid,d,SUM(-amount) FROM
      (SELECT a1 aid,amount FROM tune_moves UNION ALL SELECT a2,-amount FROM tune_moves)
    WHERE amount<0 GROUP BY aid;
    IF d<today THEN
      INSERT /*+ APPEND */ INTO daily_close(account_id,business_date,opening_balance,closing_balance,verified)
      SELECT a.account_id,d,CASE WHEN d=first_day THEN 0 ELSE a.balance END,
        a.balance+NVL(m.net,0),'Y'
      FROM account a LEFT JOIN
        (SELECT aid,SUM(amount) net FROM (SELECT a1 aid,amount FROM tune_moves UNION ALL SELECT a2,-amount FROM tune_moves) GROUP BY aid) m ON m.aid=a.account_id;
      COMMIT;
    END IF;
    MERGE INTO account a USING
      (SELECT aid,SUM(amount) net,COUNT(*) cnt,MAX(CASE WHEN amount<0 THEN k END) last_k FROM (SELECT a1 aid,amount,k FROM tune_moves UNION ALL SELECT a2,-amount,k FROM tune_moves) GROUP BY aid) m
    ON(a.account_id=m.aid) WHEN MATCHED THEN UPDATE SET a.balance=a.balance+m.net,
      a.next_sequence=a.next_sequence+m.cnt,a.last_activity_at=CASE WHEN m.last_k IS NULL THEN a.last_activity_at ELSE at_time+NUMTODSINTERVAL(m.last_k/1000,'SECOND') END;
    -- Exercise the outbox's historical insert/delete pattern, retaining only a small final backlog.
    INSERT INTO outbox(event_id,event_json,created_at)
    SELECT LPAD(tid,26,'0'),JSON_OBJECT('schema' VALUE 'saltation.log/1','event_id' VALUE LPAD(tid,26,'0'),
      'event_type' VALUE 'request.completed','occurred_at' VALUE TO_CHAR(at_time,'YYYY-MM-DD"T"HH24:MI:SSTZH:TZM'),
      'recorded_at' VALUE TO_CHAR(SYSTIMESTAMP,'YYYY-MM-DD"T"HH24:MI:SSTZH:TZM'),
      'request_id' VALUE LPAD(tid,26,'0'),'session_id' VALUE NULL,'device_id' VALUE NULL,
      'customer_no' VALUE 'C'||LPAD(CASE WHEN amount<0 THEN a1 ELSE a2 END,10,'0'),
      'account_no' VALUE '9600'||LPAD(CASE WHEN amount<0 THEN a1 ELSE a2 END,8,'0'),
      'ip' VALUE NULL,'region' VALUE NULL,'rsp_code' VALUE 'A0000','bank_rsp_code' VALUE '000',
      'channel' VALUE 'MOBILE','listener' VALUE 'PUBLIC','human' VALUE 'false' FORMAT JSON,'outcome' VALUE 'completed',
      'payload' VALUE JSON_OBJECT('transfer_id' VALUE tid)),at_time FROM tune_moves;
    DELETE FROM outbox WHERE d<today OR event_id NOT IN (SELECT LPAD(tid,26,'0') FROM tune_moves WHERE ROWNUM<=100);
    COMMIT;
    base_id := base_id+moves;
    IF TO_CHAR(d,'DD')='01' OR d=today THEN DBMS_OUTPUT.PUT_LINE('seeded through '||TO_CHAR(d,'YYYY-MM-DD')); END IF;
    d:=d+1;
  END LOOP;
  -- Zero-balance term accounts need no ledger legs; completed-date closes cover them.
  INSERT INTO account(account_id,account_no,customer_id,terms_id,subtype,status,balance,next_sequence,opened_at)
  SELECT n+rn,'20'||LPAD(n+rn,12,'0'),cid,2,'TERM',
    CASE WHEN maturity<=today THEN 'CLOSED' ELSE 'ACTIVE' END,0,1,
    FROM_TZ(CAST(first_day AS TIMESTAMP),'+09:00')
  FROM (SELECT LEVEL rn,1+10*(LEVEL-1) cid,
    first_day+30+TRUNC((LEVEL-1)*(today+365-first_day-30)/GREATEST(FLOOR(n/10)-1,1)) maturity
    FROM dual CONNECT BY LEVEL<=GREATEST(FLOOR(n/10),1)) WHERE rn<=FLOOR(n/10);
  INSERT INTO deposit_detail(account_id,linked_account_id,principal,maturity_date,closed_at,close_type)
  SELECT account_id,customer_id,100000,maturity,
    CASE WHEN maturity<=today THEN FROM_TZ(CAST(maturity AS TIMESTAMP),'+09:00') END,
    CASE WHEN maturity<=today THEN 'MATURITY' END
  FROM (SELECT a.*,first_day+30+TRUNC((account_id-n-1)*(today+365-first_day-30)/GREATEST(FLOOR(n/10)-1,1)) maturity
    FROM account a WHERE subtype='TERM');
  INSERT INTO fintech_account(fintech_use_num,account_id,user_seq_no)
  SELECT '998'||LPAD(account_id,21,'0'),account_id,LPAD(customer_id,10,'0') FROM account WHERE subtype='TERM';
  INSERT INTO account_status_history(account_id,to_status,reason,occurred_at)
  SELECT account_id,'ACTIVE','OPENED',opened_at FROM account WHERE subtype='TERM';
  INSERT INTO account_status_history(account_id,from_status,to_status,reason,occurred_at)
  SELECT account_id,'ACTIVE','CLOSED','maturity',closed_at FROM deposit_detail WHERE closed_at IS NOT NULL;
  INSERT INTO daily_close(account_id,business_date,opening_balance,closing_balance,verified)
  SELECT a.account_id,first_day+k-1,0,0,'Y' FROM account a
  CROSS JOIN (SELECT LEVEL k FROM dual CONNECT BY LEVEL<=today-first_day) WHERE a.subtype='TERM';
  INSERT INTO password_reset(customer_id,code_hash,issued_at,expires_at,used_at)
  SELECT cid,'$argon2id$v=19$m=19456,t=2,p=1$dHVuZS1yZXNldC1zYWx0ITA$Ki6s9/x/m1RaBkeVvvLSkRQDG2HokffVrI9PpTnTZZs',issued,issued+INTERVAL '5' MINUTE,issued+INTERVAL '1' MINUTE
  FROM (SELECT 1+50*(LEVEL-1) cid,
    FROM_TZ(CAST(first_day+TRUNC((LEVEL-1)*(today-first_day)/GREATEST(FLOOR(n/50),1)) AS TIMESTAMP),'+09:00') issued,
    LEVEL rn FROM dual CONNECT BY LEVEL<=GREATEST(FLOOR(n/50),1)) WHERE rn<=FLOOR(n/50);
  INSERT INTO atm_terminal(terminal_id,region,location,latitude,longitude)
  SELECT 'TUNE-ATM','SEOUL','Synthetic terminal',37.5,127 FROM dual WHERE FLOOR(n/100)>0;
  INSERT INTO atm_session(session_id,terminal_id,account_no,token_hash,started_at,expires_at,ended_at,end_reason,request_id)
  SELECT LPAD('A'||rn,26,'0'),'TUNE-ATM','9600'||LPAD(cid,8,'0'),
    LOWER(STANDARD_HASH('tune-atm-token-'||rn,'SHA256')),started,
    started+INTERVAL '3' MINUTE,started+INTERVAL '1' MINUTE,'ended',LPAD('R'||rn,26,'0')
  FROM (SELECT LEVEL rn,1+100*(LEVEL-1) cid,
    FROM_TZ(CAST(first_day+TRUNC((LEVEL-1)*(today-first_day)/GREATEST(FLOOR(n/100),1)) AS TIMESTAMP),'+09:00') started
    FROM dual CONNECT BY LEVEL<=GREATEST(FLOOR(n/100),1)) WHERE rn<=FLOOR(n/100);
  INSERT INTO payee(customer_id,bank_code,account_no,holder_name,registered_at,channel)
  SELECT customer_id,'998',account_no,'Synthetic '||recipient,at,'MOBILE'
  FROM (SELECT a.customer_id,b.account_no,b.customer_id recipient,a.opened_at at,
      ROW_NUMBER() OVER(PARTITION BY a.customer_id ORDER BY MOD(b.customer_id-a.customer_id+n,n)) rn
    FROM account a CROSS JOIN (SELECT LEVEL k FROM dual CONNECT BY LEVEL<=LEAST(n-1,4))
    JOIN account b ON b.subtype='DEMAND' AND b.customer_id=MOD(a.customer_id+k-1,n)+1
    WHERE a.subtype='DEMAND' AND a.customer_id<=LEAST(n,1000)
      AND NOT EXISTS (SELECT 1 FROM payee p WHERE p.customer_id=a.customer_id AND p.bank_code='998' AND p.account_no=b.account_no))
  WHERE rn<=LEAST(3,n-2);
  COMMIT;
END;
/
-- Explicit seed IDs must not collide with subsequent real API inserts.
ALTER TABLE customer MODIFY customer_id GENERATED BY DEFAULT AS IDENTITY (START WITH LIMIT VALUE);
ALTER TABLE account MODIFY account_id GENERATED BY DEFAULT AS IDENTITY (START WITH LIMIT VALUE);
ALTER TABLE transfer MODIFY transfer_id GENERATED BY DEFAULT AS IDENTITY (START WITH LIMIT VALUE);
ALTER TABLE counterparty MODIFY counterparty_id GENERATED BY DEFAULT AS IDENTITY (START WITH LIMIT VALUE);
DROP TABLE tune_moves PURGE;
