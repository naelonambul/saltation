-- payees.insert_payee: registers a payee and returns its id.
-- Access path: PAYEE primary key identity and PAYEE_ACTIVE_UK.
INSERT INTO payee (customer_id, bank_code, account_no, holder_name, alias, registered_at, channel, session_id)
VALUES (:customer_id, :bank_code, :account_no, :holder_name, :alias, :registered_at, :channel, :session_id)
RETURNING payee_id INTO :id
