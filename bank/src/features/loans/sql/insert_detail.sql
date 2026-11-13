-- loans.insert_detail: records principal and calendar maturity.
-- Access path: LOAN_DETAIL primary key.
INSERT INTO loan_detail (account_id, linked_account_id, principal, disbursed_at, maturity_date) VALUES (:id, :linked, :amount, :at, :maturity)
