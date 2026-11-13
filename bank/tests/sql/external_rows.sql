-- test.external_rows: verify recipient persistence.
-- Access path: TRANSFER scan by bank_tran_id then ACCOUNT_TRANSACTION_TRANSFER_IX and ACCOUNT primary key.
SELECT a.account_no, tx.amount, tx.counterparty_bank_code, tx.counterparty_account_no, tx.counterparty_name, t.kind FROM account_transaction tx JOIN transfer t ON t.transfer_id = tx.transfer_id JOIN account a ON a.account_id = tx.account_id WHERE t.bank_tran_id = :bank_tran_id
