-- 0009_products: term deposit and loan products, terms and account details.

INSERT INTO product (product_id, product_type, name) VALUES (2, 'TERM', '살테이션 정기예금')
/
INSERT INTO product (product_id, product_type, name) VALUES (3, 'LOAN', '살테이션 신용대출')
/
INSERT INTO product_terms (terms_id, product_id, version, term_months, min_amount, max_amount, early_close_ok)
VALUES (2, 2, 1, 6, 100000, 500000000, 'Y')
/
INSERT INTO product_terms (terms_id, product_id, version, term_months, min_amount, max_amount, early_close_ok)
VALUES (3, 2, 2, 12, 100000, 500000000, 'Y')
/
INSERT INTO product_terms (terms_id, product_id, version, term_months, min_amount, max_amount, early_close_ok)
VALUES (4, 2, 3, 24, 100000, 500000000, 'Y')
/
INSERT INTO product_terms (terms_id, product_id, version, term_months, min_amount, max_amount, early_close_ok)
VALUES (5, 3, 1, 12, 1000000, 50000000, 'N')
/
INSERT INTO product_terms (terms_id, product_id, version, term_months, min_amount, max_amount, early_close_ok)
VALUES (6, 3, 2, 36, 1000000, 50000000, 'N')
/
CREATE TABLE deposit_detail (
  account_id         NUMBER(12)   NOT NULL PRIMARY KEY REFERENCES account (account_id),
  linked_account_id  NUMBER(12)   NOT NULL REFERENCES account (account_id),
  principal          NUMBER(15)   NOT NULL CHECK (principal >= 1),
  maturity_date      DATE         NOT NULL,
  closed_at          TIMESTAMP(6) WITH TIME ZONE,
  close_type         VARCHAR2(10) CHECK (close_type IN ('EARLY', 'MATURITY'))
)
/
CREATE INDEX deposit_detail_maturity_ix ON deposit_detail (maturity_date, closed_at)
/
CREATE TABLE loan_detail (
  account_id         NUMBER(12)   NOT NULL PRIMARY KEY REFERENCES account (account_id),
  linked_account_id  NUMBER(12)   NOT NULL REFERENCES account (account_id),
  principal          NUMBER(15)   NOT NULL CHECK (principal >= 1),
  disbursed_at       TIMESTAMP(6) WITH TIME ZONE NOT NULL,
  maturity_date      DATE         NOT NULL
)
