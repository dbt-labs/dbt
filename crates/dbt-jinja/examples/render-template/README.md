# render-template

This demonstrates how MiniJinja can load a single template and JSON file from disk to
render a template.

```console
$ cargo run -- -c users.json -t users.html
<!doctype html>
<title>User List</title>
<ul>
  <li>1: Peter
  <li>2: John
</ul>
```