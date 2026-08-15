A standalone process wrapper around `backends/crates/sms-fake-orange`.

# This is a development/demo tool. It is not a real SMS provider.

It **impersonates Orange Cameroon's real SMS HTTP API** — the token
endpoint and the submit endpoint — and independently POSTs delivery
receipts back to whatever `POST /dlr/{providerKey}` endpoint it's told
about. Every submission it answers is fake: no SMS is ever sent to a
real handset. It exists to unlock a local or demo run of
`accepted → queued → routed → submitted → delivered` end to end without
a real Orange sandbox account (see
[#138](https://github.com/vymalo/vsms/issues/138)) — it is the
automatable complement to, and explicitly does not close,
`docs/runbooks/36-handset-gate.adoc`, which stays the real acceptance gate
for an actual handset.

**Never point a production deployment at this.** No production compose
file references this binary, and none should ever be changed to. Its own
startup log line says so loudly, every time, at `WARN`.

# Package name

This package is `sms-fake-orange-bin`, not `sms-fake-orange` — that name
belongs to the library crate this binary depends on
(`backends/crates/sms-fake-orange`), and Cargo package names must be unique
workspace-wide. Same collision, same convention as `backends/apps/sms-worker`
(package `sms-worker-bin`) — see that binary's own module doc. The
`[[bin]]` override in `Cargo.toml` is what makes the produced executable
`sms-fake-orange` regardless.

# An honestly test-shaped dependency, in a binary target

`backends/crates/sms-fake-orange` pulls in `wiremock` — built and documented as a
testing library — as a regular dependency, not a dev-dependency, and
this binary depends on that crate as a regular dependency too. That's
unusual for `app/`, which is otherwise production binaries only, and is
deliberate rather than accidental: the whole point of this package is to
run that library's fake outside of a `#[tokio::test]`. It is confined to
this one package — no other binary in `app/` depends on it.

# Example

```bash
cargo run -p sms-fake-orange-bin -- \
    --bind-addr 127.0.0.1:8090 \
    --dlr-endpoint http://127.0.0.1:8080/dlr/orange_cm \
    --sender-number +2370000
```

Then point `sms-gateway`/`sms-worker` at it with
`ORANGE_CM_BASE_URL=http://127.0.0.1:8090`.
