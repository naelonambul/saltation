-- test.accounts_tune_owner: an owned account's identifiers for plan witnesses.
SELECT a.account_id, a.customer_id, f.fintech_use_num FROM account a
JOIN fintech_account f ON f.account_id = a.account_id
WHERE a.account_no = :account_no
