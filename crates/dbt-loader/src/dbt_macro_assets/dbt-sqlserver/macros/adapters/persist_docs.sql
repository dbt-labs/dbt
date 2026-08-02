{% macro sqlserver__alter_relation_comment(relation, relation_comment) -%}
    {%- set escaped_comment = (relation_comment or '') | replace("'", "''") -%}
    {%- set relation_name = relation.identifier | replace("'", "''") -%}
    {%- set schema_name = relation.schema | replace("'", "''") -%}

    declare @relation_schema sysname = N'{{ schema_name }}';
    declare @relation_name sysname = N'{{ relation_name }}';
    declare @relation_comment nvarchar(3750) = N'{{ escaped_comment }}';
    declare @relation_type nvarchar(128);

    select
        @relation_type =
            case
                when obj.[type] = 'V' then N'VIEW'
                when obj.[type] = 'U' then N'TABLE'
            end
    from sys.objects as obj {{ information_schema_hints() }}
    inner join sys.schemas as sch {{ information_schema_hints() }}
        on sch.schema_id = obj.schema_id
    where sch.name = @relation_schema
      and obj.name = @relation_name
      and obj.[type] in ('U', 'V');

    if @relation_type is not null
    begin
        if exists (
            select 1
            from sys.extended_properties as ep {{ information_schema_hints() }}
            inner join sys.objects as obj {{ information_schema_hints() }}
                on obj.object_id = ep.major_id
            inner join sys.schemas as sch {{ information_schema_hints() }}
                on sch.schema_id = obj.schema_id
            where ep.class = 1
              and ep.minor_id = 0
              and ep.name = N'MS_Description'
              and sch.name = @relation_schema
              and obj.name = @relation_name
              and obj.[type] in ('U', 'V')
        )
        begin
            exec sys.sp_updateextendedproperty
                @name = N'MS_Description',
                @value = @relation_comment,
                @level0type = N'SCHEMA',
                @level0name = @relation_schema,
                @level1type = @relation_type,
                @level1name = @relation_name;
        end
        else
        begin
            exec sys.sp_addextendedproperty
                @name = N'MS_Description',
                @value = @relation_comment,
                @level0type = N'SCHEMA',
                @level0name = @relation_schema,
                @level1type = @relation_type,
                @level1name = @relation_name;
        end
    end;
{%- endmacro %}


{% macro sqlserver__alter_column_comment(relation, column_dict) -%}
    {%- set relation_name = relation.identifier | replace("'", "''") -%}
    {%- set schema_name = relation.schema | replace("'", "''") -%}

    {%- for column_name, column_config in column_dict.items() %}
        {%- set escaped_column_name = column_name | replace("'", "''") -%}
        {%- set escaped_comment = (column_config.get('description') or '') | replace("'", "''") -%}

    declare @schema_{{ loop.index }} sysname = N'{{ schema_name }}';
    declare @relation_{{ loop.index }} sysname = N'{{ relation_name }}';
    declare @column_{{ loop.index }} sysname = N'{{ escaped_column_name }}';
    declare @comment_{{ loop.index }} nvarchar(3750) = N'{{ escaped_comment }}';
    declare @relation_type_{{ loop.index }} nvarchar(128);

    select
        @relation_type_{{ loop.index }} =
            case
                when obj.[type] = 'V' then N'VIEW'
                when obj.[type] = 'U' then N'TABLE'
            end
    from sys.objects as obj {{ information_schema_hints() }}
    inner join sys.schemas as sch {{ information_schema_hints() }}
        on sch.schema_id = obj.schema_id
    inner join sys.columns as col {{ information_schema_hints() }}
        on col.object_id = obj.object_id
    where sch.name = @schema_{{ loop.index }}
      and obj.name = @relation_{{ loop.index }}
      and col.name = @column_{{ loop.index }}
      and obj.[type] in ('U', 'V');

    if @relation_type_{{ loop.index }} is not null
    begin
        if exists (
            select 1
            from sys.extended_properties as ep {{ information_schema_hints() }}
            inner join sys.objects as obj {{ information_schema_hints() }}
                on obj.object_id = ep.major_id
            inner join sys.schemas as sch {{ information_schema_hints() }}
                on sch.schema_id = obj.schema_id
            inner join sys.columns as col {{ information_schema_hints() }}
                on col.object_id = ep.major_id
               and col.column_id = ep.minor_id
            where ep.class = 1
              and ep.name = N'MS_Description'
              and sch.name = @schema_{{ loop.index }}
              and obj.name = @relation_{{ loop.index }}
              and col.name = @column_{{ loop.index }}
              and obj.[type] in ('U', 'V')
        )
        begin
            exec sys.sp_updateextendedproperty
                @name = N'MS_Description',
                @value = @comment_{{ loop.index }},
                @level0type = N'SCHEMA',
                @level0name = @schema_{{ loop.index }},
                @level1type = @relation_type_{{ loop.index }},
                @level1name = @relation_{{ loop.index }},
                @level2type = N'COLUMN',
                @level2name = @column_{{ loop.index }};
        end
        else
        begin
            exec sys.sp_addextendedproperty
                @name = N'MS_Description',
                @value = @comment_{{ loop.index }},
                @level0type = N'SCHEMA',
                @level0name = @schema_{{ loop.index }},
                @level1type = @relation_type_{{ loop.index }},
                @level1name = @relation_{{ loop.index }},
                @level2type = N'COLUMN',
                @level2name = @column_{{ loop.index }};
        end
    end;
    {%- endfor %}
{%- endmacro %}
