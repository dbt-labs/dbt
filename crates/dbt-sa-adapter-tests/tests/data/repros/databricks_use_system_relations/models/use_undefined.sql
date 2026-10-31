with 

source as (

    select * from {{ source('information_schema', 'tables') }}

),

renamed as (

    select
        table_catalog,
        table_schema,
        table_name,
        table_type,
        is_insertable_into,
        commit_action,
        table_owner,
        comment,
        created,
        created_by,
        last_altered,
        last_altered_by,
        data_source_format,
        storage_sub_directory,
        storage_path

    from source

)

select * from renamed