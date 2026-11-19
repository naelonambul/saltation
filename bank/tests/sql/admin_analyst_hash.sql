-- test.admin_analyst_hash: analyst seed storage only.
SELECT password_hash FROM analyst WHERE analyst_no = :analyst_no
