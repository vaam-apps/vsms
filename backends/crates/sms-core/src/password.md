Argon2id password hashing, and the one-time-password generator every
human-account provisioning path in this codebase uses (VSA-4a, restored from
`29f93c4^`).

# Why this lives in `sms-core`, not `sms-auth`

[`hash_password`] and [`verify_password`] answer "does this password match this
hash" and nothing about who may authenticate: no schema, no database, no
framework. `sms-auth::login` verifies with them and `sms-gateway provision-user`
hashes with them; `sms-api` sits *below* `sms-auth` (the dependency runs the other
way) and so could not reach a copy that lived there. A parameter choice for a
password hash is exactly the security-sensitive value this repository does not
keep two copies of, so the one place both crates already sit above holds it.

# The parameters, and why there is no pepper

`Argon2::default()`: Argon2id, version 0x13, 19 MiB of memory, 2 passes, 1 lane
(OWASP's minimum for Argon2id), a fresh random salt, 32 bytes out. The result is
a PHC string, `$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>`, which carries its own
parameters, so a hash made with other ones verifies as it was made and a future
change of the defaults does not lock anyone out.

There is **no pepper** (VSA-4 plan, decision 1): a hash exported from before #446
(`docs/runbooks/deployment.adoc`, "Keeping the OpenID Provider's rows across 0009
and 0012") has to verify after it is imported, and a pepper would make every
imported hash wrong. The dependency is `argon2` 0.6, not the 0.5 that #446
deleted: `authkestra-engine` already resolves 0.6.0, so there is one copy in the
tree, and the PHC format is the same across the two.

# What a failure looks like

[`verify_password`] answers `false` for anything that is not a valid hash (a
corrupt stored value never verifies, the fail-closed default). That has a cost the
caller must know: a value that does not *parse* returns before any Argon2 work, so
it is **faster** than a real check. `sms-auth::login`'s dummy hash is computed with
[`hash_password`] for that reason and not typed as a literal.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
