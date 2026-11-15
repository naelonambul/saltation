-- test.close_copy_leg: duplicate a ledger leg to violate transfer shape, preserving its signed amount.
-- Access path: ACCOUNT_TRANSACTION primary key and ACCOUNT_TRANSACTION_SEQ_UK.
INSERT INTO account_transaction (account_id, account_sequence, amount, balance_after, inout_type, tran_type, print_content, counterparty_bank_code, counterparty_account_no, counterparty_name, channel, terminal_id, request_id, transfer_id, reversal_of, business_date, occurred_at, recorded_at)
SELECT r.account_id, (SELECT MAX(a.account_sequence) + 1 FROM account_transaction a WHERE a.account_id = r.account_id), r.amount, r.balance_after, r.inout_type, r.tran_type, r.print_content, r.counterparty_bank_code, r.counterparty_account_no, r.counterparty_name, r.channel, r.terminal_id, r.request_id, r.transfer_id, r.reversal_of, r.business_date, r.occurred_at, r.recorded_at
FROM account_transaction r WHERE r.tran_id = :tran_id
