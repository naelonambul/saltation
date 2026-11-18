-- test.card_payment_attributes: read the authorization and optional posted debit.
SELECT JSON_OBJECT(
 'card_id' VALUE a.card_id, 'card_no' VALUE a.card_no_sent, 'krw' VALUE a.krw_amount,
 'amount' VALUE a.original_amount, 'currency' VALUE a.currency, 'merchant' VALUE a.merchant_name,
 'mcc' VALUE a.mcc, 'country' VALUE a.country, 'entry' VALUE a.entry_mode,
 'latitude_micro' VALUE (a.latitude * 1000000), 'longitude_micro' VALUE (a.longitude * 1000000),
 'kind' VALUE t.kind, 'leg' VALUE r.amount, 'sequence' VALUE r.account_sequence,
 'balance' VALUE r.balance_after, 'channel' VALUE r.channel, 'inout' VALUE r.inout_type,
 'counterparty' VALUE r.counterparty_name, 'bank' VALUE r.counterparty_bank_code,
 'counterparty_account' VALUE r.counterparty_account_no
)
FROM card_authorization a
LEFT JOIN transfer t ON t.request_id = a.request_id
LEFT JOIN account_transaction r ON r.transfer_id = t.transfer_id AND r.request_id = a.request_id
WHERE a.request_id = :id
