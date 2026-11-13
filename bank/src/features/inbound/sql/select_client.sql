-- inbound.select_client: look up a counterparty credential by client identifier.
-- Access path: COUNTERPARTY_CLIENT_UK.
SELECT counterparty_client_id, secret_hash, scope FROM counterparty_client WHERE client_id = :client_id
