-- test.card_payment_account_status: exercise dormant reactivation and closed-account decline.
UPDATE account SET status = :new_status, last_activity_at = NULL WHERE account_no = :account_number
