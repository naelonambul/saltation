-- cards.lock_card: lock the matching card before checking its status and account.
-- Access path: CARD_NO_UK. WAIT 1 bounds row lock waits.
SELECT card_id, account_id, status FROM card WHERE card_no = :card_number FOR UPDATE WAIT 1
