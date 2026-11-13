-- deposits.close_detail: records the closure instant and reason.
-- Access path: DEPOSIT_DETAIL primary key.
UPDATE deposit_detail SET closed_at = :at, close_type = :kind WHERE account_id = :id
