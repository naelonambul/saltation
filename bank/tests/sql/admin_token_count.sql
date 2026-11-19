-- test.admin_token_count: count committed tokens for one analyst.
SELECT COUNT(*) FROM access_token t JOIN analyst a ON a.analyst_id = t.subject_id
WHERE t.subject_type = 'ANALYST' AND a.analyst_no = :analyst_no
