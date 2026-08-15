Argon2id password hashing, and the one-time-password generator every
human-account provisioning path in this codebase uses.

# Why this lives in `sms-core`, not `sms-auth` (where #194 first put it)

`sms-auth::login::authenticate_user` is not this module's only caller
any more. #52/#58 add `provisionUser`, a `sms-api` procedure that lets an
`owner`/`admin` create a console account from the admin screens rather
than only from `sms-gateway provision-user`'s CLI — and `sms-api` cannot
depend on `sms-auth` (the dependency runs the other way: `sms-auth`
depends on `sms-api`, confirmed by `backends/crates/sms-auth/Cargo.toml`'s own
`sms-api.workspace = true`). Duplicating the hashing call would have
been the cheap fix — `backends/crates/sms-api/src/procedures.rs` already accepts
that tradeoff for `CLIENT_RSA_KEY_BITS`, a bare constant — but a Argon2
parameter choice is exactly the kind of security-sensitive logic this
codebase's own convention argues against duplicating (see AGENTS.md's
`#134` section on the `sha_of` test helper that hand-rolled a second
copy of a hash algorithm and silently drifted from the real one the
moment it changed). `sms-core` is the one crate already sitting below
both `sms-api` and `sms-auth` — confirmed by both crates' own
`sms-core.workspace = true` — so moving the hashing here, rather than
duplicating it, removes the drift risk entirely instead of accepting it.

`sms-auth::login` and `backends/apps/sms-gateway`'s `provision-user` CLI command
both now call the functions here directly; neither keeps its own copy.

# What moved and what didn't

[`hash_password`], [`verify_password`] and [`generate_password`] are
pure — no schema, no database, no framework dependency, matching this
crate's own existing convention (`lib.rs`'s own doc: "conventions more
than one crate has to agree on"). `sms-auth::login`'s own timing-safe
"no such user" dummy-hash construction, and everything about *who* may
authenticate or be provisioned, stays in `sms-auth`/`sms-api` — this
module only ever answers "does this password match this hash" and
"generate me a fresh one," nothing about identity or authorization.
