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

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
