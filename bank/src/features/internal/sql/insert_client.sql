-- internal.insert_client: seed one counterparty client with its secret hash and business time.
-- Access path: COUNTERPARTY_CLIENT_UK enforces client_id uniqueness.
INSERT INTO counterparty_client (client_id, secret_hash, institution_code, scope, created_at)
VALUES (:client_id, :secret_hash, :institution_code, :scope, :created_at)
