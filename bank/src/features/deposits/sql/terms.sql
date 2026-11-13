-- deposits.terms: finds eligible term product conditions.
-- Access path: PRODUCT_TERMS primary key then PRODUCT primary key.
SELECT t.term_months, t.min_amount, t.max_amount FROM product_terms t JOIN product p ON p.product_id = t.product_id
WHERE t.terms_id = :terms AND p.product_type = 'TERM'
