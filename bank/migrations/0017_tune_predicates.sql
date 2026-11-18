-- 0017_tune_predicates: newest reset lookup by customer and reset identity.
-- Original DDL: CREATE INDEX password_reset_customer_ix ON password_reset (customer_id, issued_at)
-- Restore only through a forward migration with that original DDL.
CREATE INDEX password_reset_tune_ix ON password_reset (customer_id, reset_id) ONLINE
/
DROP INDEX password_reset_customer_ix
/
ALTER INDEX password_reset_tune_ix RENAME TO password_reset_customer_ix
