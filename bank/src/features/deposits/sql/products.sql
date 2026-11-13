-- deposits.products: lists products and their ordered terms.
-- Access path: PRODUCT primary key and PRODUCT_TERMS_UK, ordered by product and terms.
SELECT p.product_id, p.product_type, p.name, t.terms_id, t.term_months, t.min_amount, t.max_amount, t.early_close_ok
FROM product p JOIN product_terms t ON t.product_id = p.product_id
ORDER BY p.product_id, t.terms_id
