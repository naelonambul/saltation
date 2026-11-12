-- test.count_transfers_btid: transfers recorded for one bank_tran_id.
SELECT COUNT(*) FROM transfer WHERE bank_tran_id = :bank_tran_id
