-- test.backfill_atm_credential: inspect lock state without reading the PIN hash.
-- Access path: ATM_CREDENTIAL primary key.
SELECT failed_count, TO_CHAR(locked_until, 'YYYYMMDDHH24MISS')
FROM atm_credential WHERE account_no = :account
