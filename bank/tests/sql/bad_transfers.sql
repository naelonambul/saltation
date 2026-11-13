-- test.bad_transfers: transfer records whose rows break the shape of their kind (spec F-21):
-- in-bank, two rows that offset; external, one debit row of the transfer's amount whose
-- counterparty is another institution.
SELECT COUNT(*) FROM (
  SELECT t.transfer_id FROM transfer t JOIN account_transaction r ON r.transfer_id = t.transfer_id
  WHERE t.kind = 'transfer'
  GROUP BY t.transfer_id, t.amount
  HAVING CASE
    WHEN COUNT(*) = 2 AND SUM(r.amount) = 0 THEN 0
    WHEN COUNT(*) = 1 AND SUM(r.amount) = -t.amount
         AND MAX(r.counterparty_bank_code) IN (SELECT institution_code FROM institution WHERE institution_type = 'BANK') THEN 0
    ELSE 1 END = 1)
