# What is vsms?

vsms is an application-to-person (A2P) SMS gateway: OTP and notification
delivery over MTN and Orange, with a provider abstraction that covers both
HTTP APIs and SMPP. Your application calls its API; vsms validates the
message, routes it to a carrier, tracks delivery and tells you what happened.

It is **Cameroon-first and extensible**. Cameroon is the default region and
the best-supported market; other countries are reached through data and
configuration rather than code changes. [Multi-country](design/multi-country.md)
records what that costs and what is deliberately left unsolved.

::: warning Status
Foundations and delivery are built, and so are retention, audit anchoring,
backup with a real restore drill, and alerting. What gates real traffic is
not code: [#36](https://github.com/vymalo/vsms/issues/36) (a real message to a
real Orange handset) and [#65](https://github.com/vymalo/vsms/issues/65) (kill
Orange in staging) need real carrier accounts and a person in Cameroon with a
phone. That is the [roadmap](roadmap.md)'s dated status as of 2026-09-17;
GitHub Issues is the live tracker.
:::

## Try it in one command

You can run all of it locally, with no carrier account:

```bash
git clone https://github.com/vaam-apps/vsms.git
cd vsms
just demo
```

That brings up a scratch Postgres, the gateway, the worker, a fake Orange,
and the admin console, wired together with a provisioned client, and a
message reaches `delivered` without a single real SMS being sent.
[Local development](runbooks/local-development.adoc) explains what it gives
you, and [Showcase](runbooks/showcase.adoc) runs the published images instead
of building from source.

## Where to start

| You want to…                                    | Start here                                                                                                                   |
| ----------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| Send SMS through vsms from your own application | [Integrating with vsms](integrating.md): credentials, the token exchange, sending, message states, webhooks                  |
| Run the whole thing locally, no carrier account | [Local development](runbooks/local-development.adoc): `just demo` and what it gives you                                      |
| Work on vsms itself                             | [First-time setup](runbooks/getting-started.adoc), then [CONTRIBUTING.md](../CONTRIBUTING.md)                                |
| Run the entire CI gate before opening a PR      | [Running the CI gate locally](runbooks/testing.adoc): `just ci`, with only `docker`, `docker compose` and `just` on the host |
| Deploy or operate it                            | [Deployment](runbooks/deployment.adoc) and the other [runbooks](runbooks/README.adoc)                                        |
| Understand why it is shaped the way it is       | [Architecture](architecture.md), the full design and every framework constraint that shaped a decision                       |

## How it is built

- **Rust** for the gateway and the worker, on
  [CrateStack](https://cratestack.dev/): schema-first, so one `.cstack` schema
  generates the model layer, policies, audit, events and the REST surface.
- **A signed transport** (COSE_Sign1 envelopes, on CrateStack's envelope
  layer). Machine callers sign every request with an Ed25519 key they
  generated and the gateway signs its answers, so there are no tokens and no
  shared secrets anywhere in the system. Staff use the console with a key their
  own browser made and a vaam admin phone (or an operator on the command line)
  vouched for: there is no password, no login and no OpenID provider.
- **PostgreSQL 16** as the only coordination mechanism: system of record,
  claim loops via optimistic CAS on `@version`, leader election via advisory
  locks, and the state machines enforced by triggers. No broker, no Redis, no
  consensus library.
- **TypeScript / Next.js** for the admin console: messages and state
  timelines, an operator dashboard, jobs, workers, providers, routes and a
  route simulator.

## What is in the repository

| Path                                           | What it is                                                                                  |
| ---------------------------------------------- | ------------------------------------------------------------------------------------------- |
| `schemas/vsms.cstack`                          | The schema: models, enums and procedures, expanded into the server by CrateStack.           |
| `backends/apps/sms-gateway/`                   | The API server binary, and the operator commands (`bootstrap`, `provision-client`, …).      |
| `backends/apps/sms-worker/`                    | The role-selectable worker binary (`--roles dispatch,scheduler,jobs,...`).                  |
| `backends/crates/`                             | The libraries: API, auth, worker, encoding, MSISDN parsing, webhooks and provider adapters. |
| `frontends/apps/admin/`, `frontends/packages/` | The Next.js admin console and its shared TypeScript packages.                               |
| `sdks/`                                        | The [Rust and Node SDKs](sdks.md).                                                          |
| `examples/`                                    | Runnable Rust and Node senders, a reference webhook receiver, and the demo's evaluator.     |
| `deploy/`                                      | Compose stack, Caddy edge, migration job, backup and restore.                               |

The [README](../README.md) has the full table, and
[OPEN_QUESTIONS.md](../OPEN_QUESTIONS.md) lists what the system does not know
the answer to yet: human decisions, unverified claims and accepted
limitations.

vsms is released under the [MIT License](../LICENSE).
