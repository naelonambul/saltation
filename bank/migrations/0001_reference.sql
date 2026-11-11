-- 0001_reference: institutions, products and terms, common codes and mappings.

CREATE TABLE institution (
  institution_code  VARCHAR2(3)    NOT NULL PRIMARY KEY,   -- bank_code_std
  name              VARCHAR2(60)   NOT NULL,
  institution_type  VARCHAR2(20)   NOT NULL CHECK (institution_type IN ('SALTATION', 'BANK'))
)
/
CREATE TABLE product (
  product_id    NUMBER(10)     NOT NULL PRIMARY KEY,
  product_type  VARCHAR2(10)   NOT NULL CHECK (product_type IN ('DEMAND', 'TERM', 'LOAN')),
  name          VARCHAR2(100)  NOT NULL
)
/
CREATE TABLE product_terms (
  terms_id        NUMBER(10)   NOT NULL PRIMARY KEY,
  product_id      NUMBER(10)   NOT NULL REFERENCES product (product_id),
  version         NUMBER(5)    NOT NULL,
  term_months     NUMBER(4),
  min_amount      NUMBER(15)   NOT NULL CHECK (min_amount >= 1),
  max_amount      NUMBER(15)   NOT NULL,
  early_close_ok  CHAR(1)      DEFAULT 'N' NOT NULL CHECK (early_close_ok IN ('Y', 'N')),
  CONSTRAINT product_terms_uk UNIQUE (product_id, version),
  CONSTRAINT product_terms_range_ck CHECK (max_amount >= min_amount)
)
/
CREATE TABLE code (
  code_group  VARCHAR2(30)   NOT NULL,
  code        VARCHAR2(30)   NOT NULL,
  label_ko    VARCHAR2(100)  NOT NULL,
  CONSTRAINT code_pk PRIMARY KEY (code_group, code)
)
/
CREATE TABLE code_map (
  code_group   VARCHAR2(30)   NOT NULL,
  code         VARCHAR2(30)   NOT NULL,
  target       VARCHAR2(20)   NOT NULL CHECK (target IN ('OPENBANKING', 'MYDATA')),
  target_value VARCHAR2(30)   NOT NULL,
  CONSTRAINT code_map_pk PRIMARY KEY (code_group, code, target),
  CONSTRAINT code_map_code_fk FOREIGN KEY (code_group, code) REFERENCES code (code_group, code)
)
