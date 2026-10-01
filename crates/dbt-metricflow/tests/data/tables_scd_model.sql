-- SCD model test tables
-- Listings as a slowly-changing dimension with validity windows.
-- listing_ids match those in fct_bookings (VARCHAR format).
-- active_from/active_to define the validity period.
CREATE TABLE dim_listings (
    listing_id VARCHAR,
    capacity INTEGER,
    is_lux BOOLEAN,
    user_id VARCHAR,
    active_from DATE,
    active_to DATE
);

INSERT INTO dim_listings VALUES
    ('l3141592', 3, true,  'u0004114', '2019-12-01', '2020-02-01'),
    ('l3141592', 4, true,  'u0004114', '2020-02-01', NULL),
    ('l5948301', 5, true,  'u0004114', '2019-12-01', NULL),
    ('l2718281', 4, false, 'u0005432', '2019-12-01', '2020-03-01'),
    ('l2718281', 6, false, 'u0005432', '2020-03-01', NULL),
    ('l9658588-incomplete', NULL, NULL, 'u1004114', '2019-12-01', NULL),
    ('l8912456-incomplete', NULL, NULL, 'u1004114', '2019-12-01', NULL);

-- dim_users_latest: users for SCD model tests.
-- Uses same schema as the simple model but a separate table.
CREATE TABLE scd_dim_users_latest (
    user_id VARCHAR,
    home_state_latest VARCHAR
);

INSERT INTO scd_dim_users_latest VALUES
    ('u0003141', 'MD'),
    ('u0003154', 'CA'),
    ('u0003452', 'HI'),
    ('u0004114', 'CA'),
    ('u0005432', 'TX'),
    ('u1004114', 'NY');

-- dim_lux_listing_id_mapping: non-SCD bridge table.
-- Maps listing_id → lux_listing_id.
CREATE TABLE scd_dim_lux_listing_id_mapping (
    listing_id VARCHAR,
    lux_listing_id VARCHAR
);

INSERT INTO scd_dim_lux_listing_id_mapping VALUES
    ('l3141592', 'll_001'),
    ('l5948301', 'll_002');

-- dim_lux_listings: second SCD table with valid_from/valid_to.
CREATE TABLE dim_lux_listings (
    lux_listing_id VARCHAR,
    is_confirmed_lux BOOLEAN,
    valid_from DATE,
    valid_to DATE
);

INSERT INTO dim_lux_listings VALUES
    ('ll_001', true,  '2019-12-01', '2020-02-01'),
    ('ll_001', true,  '2020-02-01', NULL),
    ('ll_002', false, '2019-12-01', '2020-01-01'),
    ('ll_002', true,  '2020-01-01', NULL);
