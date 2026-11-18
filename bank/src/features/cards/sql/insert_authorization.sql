-- cards.insert_authorization: store the validated authorization attributes and their request.
-- Access path: CARD_AUTHORIZATION primary key and the card foreign key index.
INSERT INTO card_authorization (
  request_id, card_id, card_no_sent, krw_amount, currency, original_amount,
  merchant_name, mcc, country, entry_mode, latitude, longitude
)
VALUES (:id, :card, :card_number, :krw, :currency, :amount, :merchant, :mcc, :country, :entry_mode_value,
        :latitude / 1000000, :longitude / 1000000)
