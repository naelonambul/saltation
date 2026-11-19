SELECT JSON_OBJECT(
    'customer_no' VALUE c.customer_no,
    'balance' VALUE a.balance,
    'opening_leg_id' VALUE l.tran_id,
    'opening_request_id' VALUE l.request_id,
    'opening_transfer_request_id' VALUE t.request_id,
    'opening_transfer_amount' VALUE t.amount,
    'opening_channel' VALUE l.channel,
    'opening_kind' VALUE t.kind,
    'opening_amount' VALUE l.amount,
    'opening_balance' VALUE l.balance_after
)
FROM account a
JOIN customer c ON c.customer_id = a.customer_id
LEFT JOIN account_transaction l ON l.account_id = a.account_id
    AND l.account_sequence = (
        SELECT MIN(first_leg.account_sequence)
        FROM account_transaction first_leg
        WHERE first_leg.account_id = a.account_id
    )
LEFT JOIN transfer t ON t.transfer_id = l.transfer_id
WHERE a.account_no = :account_no
