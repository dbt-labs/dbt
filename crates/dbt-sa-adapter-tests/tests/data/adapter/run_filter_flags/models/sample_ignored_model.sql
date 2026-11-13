-- include .render() to ignore any run time filter
select * from {{ ref('data').render() }}
