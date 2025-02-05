# Include Module

The Include module is responsible for the starter project scaffold.

# Directories

## `starter_project`
Produces the default project after running the `dbt init` command for the CLI. `dbt-cloud` initializes the project by using [dbt-starter-project](https://github.com/dbt-labs/dbt-starter-project).

# adapter.dispatch
Packages (e.g. `include` directories of adapters, any [hub](https://hub.getdbt.com/)-hosted package) can be interpreted as namespaces of functions a.k.a macros. In `dbt`'s macrospace, we take advantage of the multiple dispatch programming language concept. In short, multiple dispatch supports dynamic searching for a function across several namespaces—usually in a manually specified manner/order.

Adapters can have their own implementation of the same macro X. For example, a macro executed by `dbt-redshift` may need a specific implementation different from `dbt-snowflake`'s macro. We use multiple dispatch via `adapter.dispatch`, a Jinja function, which enables polymorphic macro invocations. The chosen implementation is selected according to what the `adapter` object is set to at runtime (it could be for redshift, postgres, and so on).

For more on this object, check out the dbt docs [here](https://docs.getdbt.com/reference/dbt-jinja-functions/adapter).

# dbt and database adapter python package interop

Let’s say we have a fictional python app named `dbt-core` with this structure

```
dbt
├── adapters
│   └── base.py
├── cli.py
└── main.py
```

`pip install dbt-core` will install this application in my python environment, maintaining the same structure. Note that `dbt.adapters` only contains a `base.py`. In this example, we can assume that base.py includes an abstract class for creating connections. Let’s say we wanted to create an postgres adapter that this app could use, and can be installed independently. We can create a python package with the following structure called `dbt-postgres`
```
dbt
└── adapters
    └── postgres
        └── impl.py
```

`pip install dbt-postgres` will install this package in the python environment, maintaining the same structure again. Let’s say `impl.py` imports `dbt.adapters.base` and implements a concrete class inheriting from the abstract class in `base.py` from the `dbt-core` package. Since our top level package is named the same in both packages, `pip` will put this in the same place. We end up with this installed in our python environment.

```
dbt
├── adapters
│   ├── base.py
│   └── postgres
│       └── impl.py
├── cli.py
└── main.py
```

`dbt.adapters` now has a postgres module that dbt can easily find and call directly. dbt and its adapters follows the same type of file structure convention. This is the magic that allows you to import `dbt.*` in database adapters, and using a factory pattern in dbt-core, we can create instances of concrete classes defined in the database adapter packages (for creating connections, defining database configuration, defining credentials, etc.)
