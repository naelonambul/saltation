-- transfers.select_external: resolve an external recipient and its bank.
-- Access path: COUNTERPARTY_ACCOUNT_UK then INSTITUTION primary key.
SELECT c.name, i.name FROM counterparty c JOIN institution i ON i.institution_code = c.institution_code
WHERE c.institution_code = :bank_code AND c.account_no = :account_no
