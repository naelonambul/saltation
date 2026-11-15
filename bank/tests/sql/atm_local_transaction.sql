-- test.atm_local_transaction: inspect the checked-out connection transaction.
-- Access path: DUAL singleton.
SELECT DBMS_TRANSACTION.LOCAL_TRANSACTION_ID FROM dual
