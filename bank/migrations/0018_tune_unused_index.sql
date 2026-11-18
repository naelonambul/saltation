-- 0018_tune_unused_index: remove a write-only lookup index after reader audit.
-- Original DDL: CREATE INDEX transaction_request_btid_ix ON transaction_request (bank_tran_id)
-- Restore only through a forward migration with that original DDL.
DROP INDEX transaction_request_btid_ix
