-- ledger.update_status: change the status of an already locked account.
-- Access path: ACCOUNT primary key.
UPDATE account SET status = :status WHERE account_id = :account_id
