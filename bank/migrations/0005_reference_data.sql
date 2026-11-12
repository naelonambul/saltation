-- 0005_reference_data: Saltation's institution code and the demand deposit product.
-- '998' is a synthetic bank_code_std chosen for Saltation (spec Open questions:
-- the value is recorded in contracts/codes and is not a real institution).

INSERT INTO institution (institution_code, name, institution_type) VALUES ('998', '살테이션은행', 'SALTATION')
/
INSERT INTO product (product_id, product_type, name) VALUES (1, 'DEMAND', '살테이션 입출금통장')
/
INSERT INTO product_terms (terms_id, product_id, version, term_months, min_amount, max_amount, early_close_ok)
VALUES (1, 1, 1, NULL, 1, 999999999999, 'N')
