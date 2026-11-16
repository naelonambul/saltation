INSERT INTO card (card_no, account_id, status, issued_at)
SELECT '0001000000000000', account_id, status, issued_at FROM card WHERE card_no = '0000000000000000'
