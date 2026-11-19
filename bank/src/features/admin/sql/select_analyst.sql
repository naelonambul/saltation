-- admin.select_analyst: find credentials by analyst number for sign-in.
-- Access path: ANALYST_NO_UK.
SELECT analyst_id, password_hash FROM analyst WHERE analyst_no = :analyst_no
