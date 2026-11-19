-- admin.insert_analyst: insert a seeded analyst with an Argon2id hash.
-- Access path: ANALYST_NO_UK enforces uniqueness.
INSERT INTO analyst (analyst_no, name, password_hash)
VALUES (:analyst_no, :display_name, :password_hash)
