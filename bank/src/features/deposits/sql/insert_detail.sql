-- deposits.insert_detail: records principal and calendar maturity.
-- Access path: DEPOSIT_DETAIL primary key.
INSERT INTO deposit_detail (account_id, linked_account_id, principal, maturity_date) VALUES (:id, :linked, :amount, :maturity)
