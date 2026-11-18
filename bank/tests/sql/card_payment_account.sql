-- test.card_payment_account: read locked-debit effects, activity, and unchanged limit totals.
SELECT JSON_OBJECT(
 'balance' VALUE a.balance, 'sequence' VALUE a.next_sequence, 'status' VALUE a.status,
 'activity' VALUE TO_CHAR(a.last_activity_at, 'YYYY-MM-DD HH24:MI:SS.FF6'),
 'reactivations' VALUE (SELECT COUNT(*) FROM account_status_history h
   WHERE h.account_id = a.account_id AND h.to_status = 'ACTIVE' AND h.from_status = 'DORMANT'),
 'limits' VALUE (SELECT NVL(SUM(d.transfer_out + d.cash_out + d.cash_in), 0)
   FROM daily_account_total d WHERE d.account_id = a.account_id)
)
FROM account a WHERE a.account_no = :account_number
