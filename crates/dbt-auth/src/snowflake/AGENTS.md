## Snowflake-specific guidance

Snowflake auth contains **intentional stubbing behavior** that must not be removed or “cleaned up”.

### Stubbed password behavior

Some Snowflake authentication methods **must provide a password to the downstream builder even when the authentication method does not actually use one**.

To support this, the code intentionally injects a stub password:

`ADBC_STUB_PASSWORD`

This occurs in flows such as:

- keypair authentication
- SSO (`externalbrowser`)
- other flows where the builder still expects a username/password shape

This stub exists to satisfy downstream driver expectations while preserving correct auth semantics.

Understand that each of these is a potentially semantically critical choice:

- removing the stubbed password
- replacing it with an empty value
- attempting to "simplify" the logic by omitting the password
- stubbing different auth flows that are in fact accept either a user-provided value or nothing

Removing or altering this behavior can break valid Snowflake authentication flows.

### OAuth exception

Unlike other auth methods, **OAuth does NOT use the stubbed user or password**.

OAuth flows intentionally avoid injecting username/password credentials and instead pass the OAuth credentials directly.

Do not introduce stubbed credentials into OAuth flows.

### Why this matters

If the stub logic is modified incorrectly, the result may:

- break authentication for existing Snowflake profiles
- prevent users from overriding credentials correctly
- cause the driver builder to reject valid configurations

These failures typically appear **only at runtime**, not at compile time.

### Human verification required

Any change affecting:

- `ADBC_STUB_PASSWORD`
- password handling in Snowflake auth flows
- OAuth credential handling
- username/password injection logic

must be explicitly flagged for **human verification before commit**.
