{{ config(
    materialized='table',
    sql_header="ALTER SESSION SET TIMEZONE = 'Asia/Kolkata';")
}}


{{ print(config.get('sql_header')) }}
select 'Hello World' as message
