-- test.atm_dormant: reproduce the dormancy job's status and old activity.
-- Access path: ACCOUNT_NO_UK.
UPDATE account SET status = 'DORMANT', last_activity_at = SYSTIMESTAMP - INTERVAL '366' DAY(3) WHERE account_no = :account
