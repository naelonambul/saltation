-- 0006_transfer_indexes: one transfer per request; lookups for transfer results.

CREATE UNIQUE INDEX transfer_request_uk ON transfer (request_id)
/
CREATE INDEX transaction_request_btid_ix ON transaction_request (bank_tran_id)
