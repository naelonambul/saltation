-- deposits.number: assigns the final term account number.
-- Access path: ACCOUNT primary key; ACCOUNT_NO_UK validates the final number.
UPDATE account SET account_no = :num WHERE account_id = :id
