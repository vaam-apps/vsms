---
layout: home
title: vsms
titleTemplate: A2P SMS gateway

hero:
  name: vsms
  text: An A2P SMS gateway
  tagline: Cameroon-first and extensible. OTP and notification delivery over Orange and MTN, with signed webhooks, no shared secrets, and PostgreSQL as the only coordination mechanism.
  image:
    src: /logo.svg
    alt: vsms
  actions:
    - theme: brand
      text: What is vsms?
      link: /what-is-vsms
    - theme: alt
      text: Send SMS through it
      link: /integrating
    - theme: alt
      text: View on GitHub
      link: https://github.com/vaam-apps/vsms

features:
  - title: OTPs and notifications
    details: A message is validated, routed to a carrier, tracked through a state machine Postgres enforces, and reported back to you by signed, at-least-once webhooks with retries and circuit breaking.
    link: /integrating
    linkText: Integrating with vsms
  - title: No shared secrets
    details: Machine callers use OAuth client_credentials with private_key_jwt. Staff sign in with authorization_code and PKCE, or through a Keycloak realm. The Rust and Node SDKs own the token lifecycle for you.
    link: /sdks
    linkText: SDKs
  - title: PostgreSQL is the only coordinator
    details: System of record, claim loops, leader election and the message and job state machines all live in PostgreSQL 16. No broker, no Redis, no consensus library.
    link: /architecture
    linkText: Architecture
  - title: The whole stack, one command
    details: "<code>just demo</code> brings up Postgres, the gateway, the worker, a fault-injecting fake Orange and the admin console, and a message reaches delivered without a single real SMS being sent."
    link: /runbooks/local-development
    linkText: Local development
---
