Provisioning a machine client for the `OpenID` Provider (VSA-3a).

One call makes the three things a token caller needs, in one transaction: an
`AppClient` (the principal a verified token resolves to; its `appc_<uuid>` is the
OAuth `client_id`), an `OauthClient` registered for `private_key_jwt` and
`client_credentials` only, and a fresh RSA keypair of which **only the public JWK
is stored**. The private key is returned once, to be written to a file, and held
nowhere else: `OauthClient` has no secret column, which is what "no shared secret
anywhere" means here.

This is the pre-#446 `provisionAppClient`'s body, moved out of the procedure.
`provisionAppClient` is now the signing-key registration (the caller keeps the
private key and sends the public one, `docs/architecture.md`, section 4.9) and
stays so; a token client is made by this function, from the `provision-client`
command, on an operator's authority. There is no HTTP form of it in VSA-3a (the
plan's `provisionTokenClient` procedure would change the contract and both SDKs,
and nothing needs it until a console can ask for one).

The scopes are checked the way `provisionAppClient` checks them: `console:pair`
is refused (only `register-client` grants it), the app must exist and be active.

**The app is read inside the transaction, and the transaction is serializable.**
`provisionAppClient` was `@isolation("serializable")`, which wraps its whole body;
this function is not a procedure, so it asks for the same thing with
`cratestack::run_in_isolated_tx` (the helper `provision-user` already uses, with
the same retry on `40001` and `40P01`). The active-app read and the two writes
therefore see one snapshot: an app deactivated between the check and the writes
makes one side retry instead of being handed a working client. A refusal
(`AppNotFound`, `AppInactive`) leaves the transaction as its value, with nothing
written.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
