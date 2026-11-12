-- ledger.lock_account: locks one account row; callers lock in ascending account_id order.
-- Access path: ACCOUNT primary key. WAIT 1 bounds lock waits (ORA-30006 on expiry).
SELECT balance, next_sequence, status, subtype FROM account WHERE account_id = :account_id FOR UPDATE WAIT 1
