-- ledger.insert_row: one account's side of a movement.
-- Access path: ACCOUNT_TRANSACTION primary key, ACCOUNT_TRANSACTION_SEQ_UK.
INSERT INTO account_transaction (
  account_id, account_sequence, amount, balance_after, inout_type, tran_type, print_content,
  counterparty_bank_code, counterparty_account_no, counterparty_name, channel, terminal_id,
  request_id, transfer_id, business_date, occurred_at)
VALUES (
  :account_id, :account_sequence, :amount, :balance_after, :inout_type, :tran_type, :print_content,
  :counterparty_bank_code, :counterparty_account_no, :counterparty_name, :channel, :terminal_id,
  :request_id, :transfer_id, :business_date, :occurred_at)
