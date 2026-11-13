-- ledger.lock_accounts: locks up to two account rows in one round trip, in ascending account_id order.
-- Access path: ACCOUNT primary key (IN-list). WAIT 1 bounds lock waits (ORA-30006 on expiry).
SELECT account_id, balance, next_sequence, status, subtype, account_no FROM account
WHERE account_id IN (:first_id, :second_id) ORDER BY account_id FOR UPDATE WAIT 1
