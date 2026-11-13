-- internal.bank_exists: validate that a client institution is a bank.
-- Access path: INSTITUTION primary key with institution_type filter.
SELECT COUNT(*) FROM institution WHERE institution_code = :code AND institution_type = 'BANK'
