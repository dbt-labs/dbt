{%- materialization seed, adapter='salesforce', supported_languages=['sql']  -%}
{%- set message -%}
Seeds are not currently supported by the Salesforce adapter.

To manually seed data into Salesforce Data 360:
  1. Login to Data 360
  2. Click on "Data Streams"
  3. Click "New"
  4. Select "File Upload", then "Next"
  5. Upload `{{ model.original_file_path }}`, then "Next"
  6. Configure the target DLO, then "Next":
    - Data Lake Object Label : set to `{{ model.alias }}`
    - Data Lake Object API Name : set to `{{ model.name }}`
    - Category : required - `Profile` is the default choice
    - Primary Key : required
  7. Configure the new Data Stream, then "Deploy"
    - Data Stream Name : can be set to anything, defaults to the filename of the seed file
    - Data Space: use `default`
{%- endset -%}

{%- do exceptions.raise_compiler_error(message) -%}
{%- endmaterialization -%}
