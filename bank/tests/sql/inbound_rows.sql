-- test.inbound_rows: inspect all ledger legs belonging to one inbound request.
-- Access path: ACCOUNT_TRANSACTION_TRANSFER_IX after TRANSFER request filter.
SELECT a.amount, a.counterparty_bank_code, a.counterparty_account_no, a.counterparty_name, t.kind
FROM transfer t JOIN account_transaction a ON a.transfer_id = t.transfer_id WHERE t.request_id = :request_id
