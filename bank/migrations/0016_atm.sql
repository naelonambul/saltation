-- 0016_atm: ATM terminals, account PIN credentials and bearer sessions.

CREATE TABLE atm_terminal (
  terminal_id VARCHAR2(20) PRIMARY KEY,
  region VARCHAR2(5) NOT NULL,
  location VARCHAR2(60 CHAR) NOT NULL,
  latitude NUMBER(9,6) NOT NULL CHECK (latitude BETWEEN 33.0 AND 38.7),
  longitude NUMBER(9,6) NOT NULL CHECK (longitude BETWEEN 124.5 AND 132.0)
)
/
CREATE TABLE atm_credential (
  account_no VARCHAR2(16) PRIMARY KEY REFERENCES account (account_no),
  pin_hash VARCHAR2(200) NOT NULL,
  failed_count NUMBER(1) DEFAULT 0 NOT NULL CHECK (failed_count BETWEEN 0 AND 3),
  locked_until TIMESTAMP(6) WITH TIME ZONE
)
/
CREATE TABLE atm_session (
  session_id VARCHAR2(26) PRIMARY KEY,
  terminal_id VARCHAR2(20) NOT NULL REFERENCES atm_terminal (terminal_id),
  account_no VARCHAR2(16) NOT NULL REFERENCES account (account_no),
  token_hash VARCHAR2(64) NOT NULL,
  started_at TIMESTAMP(6) WITH TIME ZONE NOT NULL,
  expires_at TIMESTAMP(6) WITH TIME ZONE NOT NULL,
  ended_at TIMESTAMP(6) WITH TIME ZONE,
  end_reason VARCHAR2(20) CHECK (end_reason IN ('ended', 'replaced', 'expired', 'locked')),
  request_id VARCHAR2(26) NOT NULL,
  CONSTRAINT atm_session_end_ck CHECK (
    (ended_at IS NULL AND end_reason IS NULL) OR
    (ended_at IS NOT NULL AND end_reason IS NOT NULL)
  )
)
/
CREATE UNIQUE INDEX atm_session_token_uk ON atm_session (token_hash)
/
CREATE UNIQUE INDEX atm_session_terminal_open_uk ON atm_session (
  CASE WHEN ended_at IS NULL THEN terminal_id END
)
/
CREATE INDEX atm_session_account_ix ON atm_session (account_no)
/
CREATE INDEX atm_session_open_expiry_ix ON atm_session (CASE WHEN ended_at IS NULL THEN expires_at END)
