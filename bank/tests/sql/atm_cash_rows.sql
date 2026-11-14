-- test.atm_cash_rows: inspect each posted cash movement.
-- Access path: ACCOUNT_TRANSACTION request index, TRANSFER primary key.
SELECT JSON_OBJECT('amount' VALUE a.amount, 'balance_after' VALUE a.balance_after,
 'inout_type' VALUE a.inout_type, 'tran_type' VALUE a.tran_type, 'print_content' VALUE a.print_content,
 'channel' VALUE a.channel, 'terminal_id' VALUE a.terminal_id,
 'counterparty_bank_code' VALUE a.counterparty_bank_code,
 'counterparty_account_no' VALUE a.counterparty_account_no, 'counterparty_name' VALUE a.counterparty_name,
 'kind' VALUE t.kind, 'transfer_id' VALUE t.transfer_id)
FROM account_transaction a JOIN transfer t ON t.transfer_id = a.transfer_id WHERE a.request_id = :id
