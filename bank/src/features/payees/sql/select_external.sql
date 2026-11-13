-- payees.select_external: the name of an external counterparty account.
-- Access path: COUNTERPARTY_ACCOUNT_UK.
SELECT name FROM counterparty WHERE institution_code = :bank_code AND account_no = :account_no
