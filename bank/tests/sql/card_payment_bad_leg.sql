-- test.card_payment_bad_leg: reverse a fixture leg's sign for the daily-close audit witness.
UPDATE account_transaction SET amount = -amount WHERE request_id = :id
