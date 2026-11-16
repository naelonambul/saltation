INSERT INTO card_authorization (request_id, card_no_sent, krw_amount, currency, original_amount, merchant_name, mcc, country, entry_mode, latitude, longitude)
VALUES ('card-migration-request', '0000000000000000', 1, 'KRW', 1, 'Merchant', '1234', 'KR', :1, TO_NUMBER(:2), TO_NUMBER(:3))
