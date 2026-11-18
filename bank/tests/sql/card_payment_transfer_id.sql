-- test.card_payment_transfer_id: read the card transfer ID using its unique request ID.
SELECT transfer_id FROM transfer WHERE request_id = :id
