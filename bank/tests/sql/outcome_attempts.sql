-- test.outcome_attempts: rejection persistence witness.
-- Access path: primary keys or full test snapshot.
SELECT json_object(*), response_json FROM transaction_request WHERE request_id = :id OR reused_request_id = :reused ORDER BY request_id
