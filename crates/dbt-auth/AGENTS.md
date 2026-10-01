# AGENTS.md

This crate is not a generic config parser.
It encodes adapter authentication semantics and preserves compatibility-sensitive behavior.

Agent-generated changes must prioritize **semantic correctness**, **compatibility**, and the existing **zero-copy / borrowed-data design**.

Mistakes here can silently change how authentication inputs are interpreted, which can break internal systems and platform behavior.
Changes must be treated as **semantic**, not stylistic.

---

## Primary rule

Do not make “cleanup” changes that widen ownership, flatten distinctions, or generalize auth inputs unless explicitly required.

`AuthIR` enums and config accessors encode **semantic truth**, not convenience structure.

Changes that simplify types or normalize values early are often **behavior regressions**, even if the code compiles.

---

## Core invariants

### 1. Borrowed data is intentional

Auth parsing and IR are designed to preserve borrowed data.

Preferred shapes:
- `&str`
- `Option<&str>`
- `Cow<'a, str>` only when normalization is required

Avoid introducing:
- `String`
- `Option<String>`
- `.to_string()`
- `.into_owned()`
- `.clone()`

unless required at a **final external boundary**.

Ownership widening here often destroys important distinctions in the input data.

---

### 2. Normalize late

Preserve original values as long as possible.

String conversion and owned allocation should happen **only at the boundary where a downstream API requires it**.

Do not normalize early simply to make intermediate code easier.

Early normalization can erase differences between values that were originally strings vs values that were coerced.

---

### 3. Accessor choice is semantic

Config accessors intentionally encode input behavior.

- `get_str` → field must be an actual YAML string
- `get_string` → field may come from numbers or booleans and be normalized to text

Do **not** replace `get_string` with `get_str` unless the change intentionally narrows accepted input shapes.

Example:

A boolean field may appear as:
true
"true"


Both are valid and must continue to work unless explicitly changed.

---

### 4. Preserve compatibility behavior

Existing profile shapes, legacy keys, and value interpretations must remain stable.

Do not silently narrow accepted input forms.

Many values may appear as either:
- native YAML types
- string equivalents

Both must remain valid unless a compatibility change is explicitly requested.

---

### 5. Auth enum growth must reflect real semantics

Do not modify enums mechanically.

Before changing auth enums, classify the change as:

1. new auth family
2. subtype of an existing family
3. platform/engine specialization

Apply:

- (1) horizontal growth is appropriate
- (2) vertical growth is preferred
- (3) vertical growth is usually preferred

Top-level variants represent **distinct authentication contracts**.

Nested enums represent **refinements within a family**.

Do not flatten multiple auth families into generalized structs with many optional fields.

---

## Risks

Incorrect changes in this crate can:

- alter how configuration values are interpreted
- erase distinctions between borrowed vs normalized values
- silently break authentication flows
- create compatibility regressions in adapter behavior
- destabilize internal systems and platform components

These failures may not appear as compile errors and may only surface at runtime.

---

## Human verification is required

Agents **must not assume correctness** when modifying this crate.

Before proposing or finalizing code, clearly report:

- whether any borrowed fields became owned
- whether `get_string` / `get_str` behavior changed
- whether accepted input shapes changed
- whether enum structure changed
- whether any values are now normalized or coerced differently

If any of the above occurs, the agent must explicitly state:

**”Human verification is required before committing this change.”**

Do not present such changes as harmless refactors. Prompt the user to run the
live smoke tests in `crates/dbt-auth-tests` — see its README for setup.
