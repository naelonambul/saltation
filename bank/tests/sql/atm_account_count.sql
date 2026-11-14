-- test.atm_account_count: prove rejected seeding left no account.
-- Access path: ACCOUNT account number unique index.
SELECT COUNT(*) FROM account WHERE account_no = :account_no
