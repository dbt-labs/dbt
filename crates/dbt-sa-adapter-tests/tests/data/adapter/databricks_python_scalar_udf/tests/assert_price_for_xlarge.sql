select result
from (select {{ function('price_for_xlarge') }}(100.0) as result)
where result != 200.0
