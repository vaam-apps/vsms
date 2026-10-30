The one question `provision-client` and `rotate-signing-key` ask before they
touch the database: does this deployment serve `machine_tokens`?

Both mint credentials the `OpenID` Provider accepts, and a deployment that mounts no
provider (the method not permitted, or `op:` not written) has nothing to accept
them. Minting one anyway would leave a client and a key that look provisioned and
that nothing honours, which is the kind of quiet half-success this repository
refuses. The answer is the same function the gateway's router and discovery
document use (`AuthMethods::machine_tokens_served`), so the commands and the
server cannot disagree. The refusal leads with `auth_method_disabled`, the word the
disabled `hmac_webhooks` 422 leads with, and names the way on: `register-client`
registers a signing key, which every deployment serves.

# `rotate-signing-key` asks a wider question (VSA-4a)

[`Provider`] is true when the provider is mounted for **either** method
(`AuthMethods::op_served`). The one signing key signs a machine's access token and a
person's `id_token`, so a deployment that serves only the sign-in still has a key
to rotate, and `serve` still refuses to start without one. `provision-client`
keeps asking [`MachineTokens`]: it mints a machine's credential, which only
`machine_tokens` honours.

# `human_login` (VSA-4a)

[`HumanLogin`] is the same question for the sign-in: `seed-console-client`,
`provision-user --password` and `bootstrap --owner-password-out` mint a console
client row or a password that only a served `human_login` honours, so a
deployment that does not serve it (the method not permitted, the `human_login:`
block not written) refuses them before the database is touched, with
`auth_method_disabled` and the way on (`pair-console-key`). The answer is
`AuthMethods::human_login_served`, the function the router, the grant gate and the
boot check use.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
