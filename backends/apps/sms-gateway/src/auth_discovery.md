`GET /.well-known/vsms-auth`: which ways of authenticating this deployment
serves (VSA-1).

```json
{ "methods": ["keypair", "hmac_webhooks"] }
```

It is `auth.methods` resolved (`sms_config::AuthMethods::served`): the permitted
methods this release has code for and, for `machine_tokens`, whose `op:` block is
written. So the absent default answers two of the four on a deployment with no
`op:` (`machine_tokens` is permitted and not served; `human_login` is permitted and
served by nothing until VSA-4), three with one, and vaam's `[keypair]` answers one. A console or an SDK reads
this instead of a second switch that could disagree with the server's.

- **Unauthenticated and outside the envelope**, like `/healthz` and `/readyz`:
  `serve` merges it beside them, after `sms_api::router` has applied its layers
  to its own routes. A client has to be able to ask before it knows how to sign,
  and the answer is a list of public names: nothing secret and nothing per caller.
- **It lists what the deployment offers, not what a caller may use.** An app
  with no `hmac` endpoint gets nothing from `hmac_webhooks` being listed.
- Resolved once at boot: changing `auth.methods` is a restart, like every other
  key in the YAML.
- vpay serves the same document at `/.well-known/vpay-auth`, with the same shape.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
