-- test.transfer_flag: verify recipient persistence.
-- Access path: TRANSFER scan filtered by bank_tran_id.
SELECT payee_registered FROM transfer WHERE bank_tran_id = :bank_tran_id
