CREATE TABLE account_month_txns (
    ds DATE,
    ds_partitioned DATE,
    account_id VARCHAR,
    account_month VARCHAR,
    txn_count INTEGER,
    txns_value INTEGER
);

INSERT INTO account_month_txns VALUES
    ('2020-01-01', '2020-01-01', 'a0', 'JANUARY', 3, 300),
    ('2020-01-01', '2020-01-01', 'a1', 'JANUARY', 2, 200),
    ('2020-01-01', '2020-01-01', 'a2', 'JANUARY', 5, 500),
    ('2020-02-01', '2020-02-01', 'a0', 'FEBRUARY', 4, 400),
    ('2020-02-01', '2020-02-01', 'a1', 'FEBRUARY', 5, 250),
    ('2020-03-01', '2020-03-01', 'a0', 'MARCH', 5, 1000),
    ('2020-03-01', '2020-03-01', 'a1', 'MARCH', 6, 360),
    ('2020-04-01', '2020-04-01', 'a0', 'APRIL', 4, 600),
    ('2020-04-01', '2020-04-01', 'a1', 'APRIL', 10, 1000);

CREATE TABLE bridge_table (
    ds_partitioned DATE,
    account_id VARCHAR,
    customer_id VARCHAR,
    extra_dim VARCHAR
);

INSERT INTO bridge_table VALUES
    ('2020-01-01', 'a0', '0', 'lux'),
    ('2020-01-01', 'a1', '2', 'not_lux'),
    ('2020-01-02', 'a0', '1', 'lux'),
    ('2020-01-04', 'a2', '3', 'super_lux');

CREATE TABLE customer_other_data (
    customer_id VARCHAR,
    country VARCHAR,
    customer_third_hop_id VARCHAR,
    acquired_ds VARCHAR
);

INSERT INTO customer_other_data VALUES
    ('0', 'turkmenistan', 'another_id0', '2020-01-01'),
    ('1', 'paraguay', 'another_id1', '2020-01-02'),
    ('2', 'myanmar', 'another_id2', '2020-01-03'),
    ('3', 'djibouti', 'another_id3', '2020-01-04');

CREATE TABLE customer_table (
    ds_partitioned DATE,
    customer_id VARCHAR,
    customer_name VARCHAR,
    customer_atomic_weight VARCHAR
);

INSERT INTO customer_table VALUES
    ('2020-01-01', '0', 'thorium', '90'),
    ('2020-01-01', '2', 'tellurium', '52'),
    ('2020-01-02', '1', 'osmium', '76'),
    ('2020-01-04', '3', 'einsteinium', '99');

CREATE TABLE third_hop_table (
    customer_third_hop_id VARCHAR,
    value VARCHAR,
    third_hop_ds VARCHAR
);

INSERT INTO third_hop_table VALUES
    ('another_id0', 'citadel', '2020-01-01'),
    ('another_id1', 'virtu', '2020-01-02'),
    ('another_id2', 'two sigma', '2020-01-03'),
    ('another_id3', 'jump', '2020-01-04');

