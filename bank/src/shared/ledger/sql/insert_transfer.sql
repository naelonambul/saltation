-- ledger.insert_transfer: one money movement.
-- Access path: TRANSFER primary key (identity), TRANSFER_REQUEST_UK.
INSERT INTO transfer (kind, request_id, bank_tran_id, amount, occurred_at, payee_registered)
VALUES (:kind, :request_id, :bank_tran_id, :amount, :occurred_at, :payee_registered)
RETURNING transfer_id INTO :id
