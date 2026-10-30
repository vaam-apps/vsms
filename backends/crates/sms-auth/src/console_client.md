The console's `OauthClient` row: the one public client a person signs in through
(VSA-4a).

The configuration (`human_login.console_client`) names the client id and the
redirect URIs; the **row** is what the provider actually reads. They can drift: an
operator edits the file and forgets the command, or a row left from before this
release carries a grant it must not. So the gateway checks the row rather than
trusting it, at boot ([`check_console_client`]), and one command makes it right
([`seed_console_client`], `sms-gateway seed-console-client`).

# What a correct row is

Public client (`tokenEndpointAuthMethod = none`, no key, no `AppClient`), grants
exactly `authorization_code`, scopes exactly `openid profile`, redirect URIs
exactly the configured list, PKCE required, active. [`mismatches`] names every
difference, not only the first, so an operator fixes it in one run.

**The refresh grant is the one the check exists for.** The row `seed-console-client`
wrote before #446 listed `authorization_code refresh_token`. A deployment that
carried that row across (the runbook's export and import) would give a person a
30-day credential for the asking, and nothing in the OP's own configuration would
say so. `serve` therefore refuses to start on it, naming the command.

# A machine client's row is never taken over

[`machine_conflicts`] picks out the three differences that mean the row is not a
console row at all: a public key, an `AppClient` link, or `private_key_jwt` as the
auth method. When the row with the configured id has any, [`seed_console_client`]
and [`check_console_client`] both answer [`ConsoleClientError::MachineClient`],
naming which, and write nothing. Reconciling would strip a service's credentials
and turn it into the console, cutting the service off silently; the repair this
command offers is for a *stale console row*, and it is told apart from a machine's
by exactly those markers. The error says to choose another
`human_login.console_client.client_id` (or remove the client deliberately) and does
not offer the command, because the command would refuse it too.

# Seeding reconciles

Creating when missing, updating when different, leaving alone when it matches. The
old command did "create, or say it exists and change nothing", which is how a
changed redirect URI never reached the row. The update sets only the columns the
rules above cover; it never touches `clientId`.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
