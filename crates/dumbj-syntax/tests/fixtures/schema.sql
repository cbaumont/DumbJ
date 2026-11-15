-- Orders schema
CREATE TABLE customers (
    id BIGINT PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

/* Index for lookups by name. */
CREATE INDEX idx_customers_name ON customers (name);

INSERT INTO customers (id, name) VALUES (1, 'Ada'), (2, 'Grace');

SELECT c.name, COUNT(o.id) AS orders, 3.5 AS ratio
FROM customers c
LEFT JOIN orders o ON o.customer_id = c.id
WHERE c.name <> 'Bob' AND c.id IN (1, 2, 3)
GROUP BY c.name
ORDER BY orders DESC;
