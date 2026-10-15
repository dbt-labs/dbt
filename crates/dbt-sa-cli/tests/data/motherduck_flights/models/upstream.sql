select i as id, i % 3 as grp
from generate_series(1, 100) as source(i)
