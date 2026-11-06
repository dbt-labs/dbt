{#
	For more information on how this XML trick works with splitting strings, see https://www.sqlservertips.com/sqlservertip/1771/splitting-delimited-strings-using-xml-in-sql-server/
	On Azure SQL and SQL Server 2019, we can use the string_split function instead of the XML trick.
	But since we don't know which version of SQL Server the user is using, we'll stick with the XML trick in this adapter.
	However, since the XML data type is not supported in Synapse, it has to be overridden in that adapter.

    To adjust for negative part numbers, aka 'from the end of the split', we take the position and subtract from last to get the specific part.
    Since the input is '-1' for the last, '-2' for second last, we add 1 to the part number to get the correct position.
#}

{% macro sqlserver__split_part(string_text, delimiter_text, part_number) %}
    {% if part_number >= 0 %}
        LTRIM(CAST(('<X>'+REPLACE({{ string_text }},{{ delimiter_text }} ,'</X><X>')+'</X>') AS XML).value('(/X)[{{ part_number }}]', 'VARCHAR(128)'))
    {% else %}
        LTRIM(CAST(('<X>'+REPLACE({{ string_text }},{{ delimiter_text }} ,'</X><X>')+'</X>') AS XML).value('(/X)[position() = last(){{ part_number }}+1][1]', 'VARCHAR(128)'))
    {% endif %}
{% endmacro %}
