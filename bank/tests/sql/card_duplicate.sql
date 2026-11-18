INSERT INTO card (card_no, account_id, status, issued_at)
SELECT card_no, account_id, status, issued_at FROM card WHERE card_no = '0000000000000000'
