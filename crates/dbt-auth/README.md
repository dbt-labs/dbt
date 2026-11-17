# Public authentication verification

This public fork does not include a `dbt-auth-tests` crate or a generic live
provider-auth suite. For Redis and Redis-backed Flight SQL, use the runnable
[public auth verification guide](https://github.com/redis/redis-dbt-adapter/blob/5b210c215143ebc74171587f30ce7f5529f3ce5f/v2/AUTH_VERIFICATION.md)
in `redis/redis-dbt-adapter`. It specifies the dedicated fixture, native/Flight
client configuration, exact commands, expected positive/negative cases, source
and runtime receipts, and human review record. Use integration revision
`5b210c215143ebc74171587f30ce7f5529f3ce5f`, which includes both probe transports.

From this engine checkout, the Redis profile/schema unit checks are:

```sh
cargo test -p dbt-auth --lib redis_adbc
cargo test -p dbt-schemas --lib redis_adbc_tests
cargo build -p dbt-sa-cli
```

Those unit tests are not live verification. The ignored `live_driver_query`
probe only passes URI/username/password; it does not configure the profile's
explicit CA option. Use the guide's trusted-TLS native CLI probes instead of
disabling certificate verification to make that ignored test pass.

The integration checkout provides these public live entry points, accepting the
absolute path of the executable built from the engine checkout under review:

```sh
python3 v2/flight_security.py --transport redis --dbt /absolute/path/to/target/debug/dbt
python3 v2/flight_security.py --transport flightsql --dbt /absolute/path/to/target/debug/dbt
```

Run them from the integration repository after the guide's fixture/driver/Flight
setup. The direct probe checks explicit credential precedence, rejected password
and CA, password redaction, no-fetch session preservation, and bounded CLI
cancellation/fresh recovery. The Flight probe checks the separate client hop's
credentials/CA, redaction and session/cancellation behavior; profile unit tests
check the restricted URI and client-option contract. The fixture independently
verifies TLS/auth, Search, topology and read/write/delete.

These commands cover Redis and Flight only. Other auth providers require their
own credentials, setup and provider-specific verification; an unavailable
fixture or skipped test is not a pass. Do not broaden the Redis/Flight result
into a claim about other providers or every deployment.

Before proposing compatibility-sensitive auth changes, report all five impacts
from `AGENTS.md`: borrowed/owned data, config accessors, accepted input shapes,
auth enum structure, and normalization/coercion. Ask the human reviewer to run
the applicable probes and record who executed/reviewed each case, exact source
revisions, commands/results, sanitized receipts, unrun cases, and the human's
review decision/date/scope. Leave human fields unfilled until supplied by the
actual reviewer. Automated logs and a maintainer merge do not establish that a
human ran a particular live test. This documentation does not waive the human
verification requirement for auth-code changes.
