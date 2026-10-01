CREATE TABLE orders (
  order_id INTEGER,
  customer_id INTEGER,
  ordered_at DATE,
  amount DOUBLE,
  status VARCHAR
);

INSERT INTO orders VALUES
  (1, 101, DATE '2024-01-01', 100.0, 'completed'),
  (2, 101, DATE '2024-01-01', 50.0, 'pending'),
  (3, 102, DATE '2024-01-02', 80.0, 'completed'),
  (4, 103, DATE '2024-01-03', 120.0, 'completed');
