# invalid-value

Demonstrates the behavior of the engine with regards to invalid values.  Invalid
values are values that crate a serde error during conversion.  MiniJinja will
defer that error until the value is interacted with at runtime.
