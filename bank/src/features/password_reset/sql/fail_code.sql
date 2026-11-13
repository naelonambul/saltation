-- password_reset.fail_code: counts an incorrect live reset attempt.
-- Access path: PASSWORD_RESET primary key.
UPDATE password_reset SET failed_count = failed_count + 1 WHERE reset_id = :reset_id
