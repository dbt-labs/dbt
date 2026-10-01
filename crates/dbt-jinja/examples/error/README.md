# error

This example demonstrates the built-in error rendering support for better
template debuggability

```console
$ cargo run
template error: could not render include: error in "include.txt" (in hello.txt:8)
---------------------------------- hello.txt ----------------------------------
   5 |             {% with foo = 42 %}
   6 |               {{ range(10) }}
   7 |               {{ other_seq|join(" ") }}
   8 >               {% include "include.txt" %}
     i                  ^^^^^^^^^^^^^^^^^^^^^ could not render include
   9 |             {% endwith %}
  10 |           {% endwith %}
  11 |         {% endfor %}
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
Referenced variables: {
    foo: 42,
    other_seq: [
        0,
        1,
        2,
        3,
        4,
    ],
    range: minijinja::functions::builtins::range,
}
-------------------------------------------------------------------------------

caused by: invalid operation: tried to use + operator on unsupported types number and string (in include.txt:1)
--------------------------------- include.txt ---------------------------------
   1 > Hello {{ item_squared + bar }}!
     i          ^^^^^^^^^^^^^^^^^^ invalid operation
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
Referenced variables: {
    bar: "test",
    item_squared: 4,
}
-------------------------------------------------------------------------------
```
