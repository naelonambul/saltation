-- password_reset.use_code: marks a reset code as consumed.
-- Access path: PASSWORD_RESET primary key.
UPDATE password_reset SET used_at = :now WHERE reset_id = :reset_id
