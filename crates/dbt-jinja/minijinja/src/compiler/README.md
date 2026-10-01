# TypeChecker for Minijinja

This module(mainly `typemeta.rs`) implements a static type checking system for Minijinja.

---

## Supported Types

The type system currently supports the following types:

| Type              | Description                                     |
| ----------------- | ----------------------------------------------- |
| `string`          | UTF-8 text string                               |
| `integer`         | Whole number                                    |
| `float`           | Floating-point number                           |
| `bool`            | Boolean (`true`/`false`)                        |
| `bytes`           | Raw byte array                                  |
| `list[...]`       | Sequence with homogeneous element types         |
| `map`             | Key-value object                                |
| `iterable`        | Iterable object                                 |
| `plain`           | Low-level object without specific structure     |
| `none`            | Explicit absence of value                       |
| `undefined`       | Variable declared but not assigned              |
| `invalid`         | Used for ill-typed values                       |
| `relation_object` | A relation object                               |
| `adapter`         | An adapter                                      |
| `value`           | Unknown or dynamic type (top type)              |
| `kwargs`          | Keyword arguments                               |
| `frame`           | A stack frame                                   |
| `function`        | A function                                      |

---

## Type Checking Logic

This type checker performs static analysis on a sequence of VM-style `Instruction`s. The logic follows these key steps:

### 1. **Stack-Based Type State**

Each instruction modifies a stack of type sets, simulating the operand stack at runtime.

### 2. **Control Flow Sensitive Analysis**

The control flow graph (CFG) of the instruction sequence is computed, and type states are propagated across basic blocks with merge points performing type union.

### 3. **Local Variable Tracking**

A symbol table tracks local variable types. During control flow joins, types of variables from different branches are unioned.

---

## Function and Macro Signatures

We support typecheck of macro signatures(defined in `.sql` files) and internal rust function signatures. 

A signature should be like:
```
function_name(type1, type2, ...) -> return_type
```

If a macro is called without a registered signature, the type checker will emit a warning.

---

## Integration Example

To typecheck a project, simply run:
```
dbt jinja-check
```
on your root path of the project.
