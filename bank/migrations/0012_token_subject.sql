-- 0012_token_subject: finds a subject's tokens (revoking a customer's tokens at a password reset).

CREATE INDEX access_token_subject_ix ON access_token (subject_id, subject_type)
