-- 0010_rollover: daily rollover tracking, close verification and automated transfers.

ALTER TABLE sim_clock ADD (rolled_through DATE)
/
CREATE TABLE rollover_run (
  business_date  DATE          NOT NULL,
  job            VARCHAR2(30)  NOT NULL,
  started_at     TIMESTAMP(6) WITH TIME ZONE DEFAULT SYSTIMESTAMP NOT NULL,
  finished_at    TIMESTAMP(6) WITH TIME ZONE,
  CONSTRAINT rollover_run_pk PRIMARY KEY (business_date, job)
)
/
CREATE TABLE daily_close (
  account_id       NUMBER(12)   NOT NULL REFERENCES account (account_id),
  business_date    DATE         NOT NULL,
  opening_balance  NUMBER(15)   NOT NULL,
  closing_balance  NUMBER(15)   NOT NULL,
  verified         CHAR(1)      NOT NULL CHECK (verified IN ('Y', 'N')),
  mismatch_detail  VARCHAR2(400 CHAR),
  recorded_at      TIMESTAMP(6) WITH TIME ZONE DEFAULT SYSTIMESTAMP NOT NULL,
  CONSTRAINT daily_close_pk PRIMARY KEY (account_id, business_date)
)
/
CREATE INDEX daily_close_date_ix ON daily_close (business_date, verified)
/
ALTER TABLE transfer ADD (payee_registered CHAR(1) CHECK (payee_registered IN ('Y', 'N')))
/
ALTER TABLE transfer MODIFY (request_id NULL)
/
ALTER TABLE account_transaction MODIFY (request_id NULL)
