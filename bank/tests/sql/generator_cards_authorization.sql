SELECT JSON_OBJECT(
    'account_no' VALUE a.account_no,
    'customer_no' VALUE c.customer_no,
    'card_no' VALUE p.card_no_sent,
    'currency' VALUE p.currency,
    'amount' VALUE p.original_amount,
    'krw_amount' VALUE p.krw_amount,
    'merchant_name' VALUE p.merchant_name,
    'mcc' VALUE p.mcc,
    'country' VALUE p.country,
    'entry_mode' VALUE p.entry_mode,
    'latitude_micro' VALUE (p.latitude * 1000000),
    'longitude_micro' VALUE (p.longitude * 1000000),
    'decline_reason' VALUE JSON_VALUE(q.response_json, '$.decline'),
    'prior_payments' VALUE (
        SELECT COUNT(*)
        FROM account_transaction earlier
        JOIN transfer t ON t.transfer_id = earlier.transfer_id
        JOIN card_authorization prior_payment ON prior_payment.request_id = earlier.request_id
        WHERE earlier.account_id = card.account_id
          AND t.kind = 'card_payment'
          AND earlier.amount < 0
          AND prior_payment.merchant_name = p.merchant_name
          AND earlier.occurred_at < q.occurred_at
    )
)
FROM card_authorization p
JOIN transaction_request q ON q.request_id = p.request_id
JOIN card ON card.card_id = p.card_id AND card.card_no = p.card_no_sent
JOIN account a ON a.account_id = card.account_id
JOIN customer c ON c.customer_id = a.customer_id
WHERE p.request_id = :id
