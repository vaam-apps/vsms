`authkestra_op::OpStore` implementations backed by `CrateStack` delegates.

Two pieces, matching the two things `authkestra-op` needs to know that only
this database has:

- [`SmsClientStore`] — `ClientStore::find_client`, reading `OauthClient`.
- [`SmsClientAssertionStore`] — `ClientAssertionStore::record_jti`,
  reading and writing `ClientAssertion`.

Both exist because R1 says all data access goes through `CrateStack`
delegates, never raw `sqlx` — `authkestra-op` ships a `sqlx_store.rs` of
its own, but adopting it would mean bypassing row-level policy, `@@audit`
and `@@emit`, which is exactly what R1 exists to prevent.

Neither type mounts a router or owns a connection pool — `sms-auth` links
`sms-api` for the expanded schema and nothing else. Wiring these into
`authkestra_op::CompositeOpStore` and the OP router itself is #20; wiring
their output into an `AuthProvider` is #21.

# Why `record_jti` reads `db_sqlstate()` rather than pre-checking

[`SmsClientAssertionStore::record_jti`]'s replay check is `create` plus
catching `23505`, not a `SELECT` followed by an `INSERT` — the pre-check
form races, and `upsert` does not exist when the `@id` carries a default
(§2.0). That makes it dependent on the driver's SQLSTATE surviving the
framework's sqlx→`CratestackError` conversion.

For the whole of `cratestack-sqlx` `=0.5.0`–`=0.5.2` it did not: every
generated write mapped through `CratestackError::Database(error.to_string())`,
discarding SQLSTATE and constraint before application code saw them, so
`db_sqlstate()` was `None` on every database-rejected write. Filed as
[cratestack/cratestack#267](https://github.com/cratestack/cratestack/issues/267),
tracked here as [vymalo/vsms#87](https://github.com/vymalo/vsms/issues/87),
**fixed in `cratestack-sqlx` 0.6.0** — all twelve write paths now route
through `cratestack_error_from_sqlx`.

`tests/live_postgres.rs`'s `record_jti_is_true_once_and_false_on_replay`
was written to assert the correct behaviour while that bug was live, and
left failing rather than weakened, so it would go green the moment the
pin moved. It does. Keep it, and the sibling assertions in
`sms-api`'s `tests/errors_live_postgres.rs`: they are the only coverage
that can see this class of regression, since a hand-constructed
`CratestackError::DatabaseTyped` never exercises the conversion, and
`cargo build` / `cratestack check` stay green through it either way.
