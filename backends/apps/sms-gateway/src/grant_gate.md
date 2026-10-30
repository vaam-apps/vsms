The grant gate in front of `/token` (VSA-4a): a `grant_type` is served only when
the method that owns it is.

`authkestra-op`'s token handler dispatches on the request's `grant_type` and
checks the **client's** registration, never the provider's `grant_types_supported`
list (that list only feeds the discovery document). So on its own it would run
`client_credentials` for a machine client on a deployment that serves only the
sign-in, and `authorization_code` on one that serves only machines, whatever the
discovery document said. This middleware makes the list true:

| `grant_type`                                     | Served when                           |
| ------------------------------------------------ | ------------------------------------- |
| `client_credentials`                             | `machine_tokens` is served            |
| `authorization_code`                             | `human_login` is served               |
| `refresh_token`, `device_code`, token exchange, any other | **never**                    |

A grant that is not served is `400 {"error": "unsupported_grant_type"}` (RFC 6749,
section 5.2), `Cache-Control: no-store`. The refresh grant is the one the owner
ruled out (*"Never ever should refresh tokens be used in this system."*, 2026-10-09):
the console client is not registered for it, the refresh slot of the store refuses
to hold one (`sms_auth::op::RefusingRefreshStore`), and this is the check that
answers the request with a word a client can act on instead of the handler's
`unauthorized_client`.

# Why it sits outside the rate limiter

`op.rs` layers it **outermost**, so a request for a grant this deployment does not
serve is refused before it takes a token from any `client_id`'s bucket. Otherwise
anyone could drain a victim's bucket with requests that were never going to be
honoured. The cost is one extra buffering of a body capped at 64 KiB, which the
rate limiter does anyway.

# What it refuses to guess

A body with no `grant_type`, with an empty one, with **two** (the handler's own form
parser refuses a repeated field, and a gate must never read a different value than
the handler would), or that cannot be read within the cap, is `invalid_request`.
It is refused, never passed on to be read differently downstream. A grant type is
compared exactly: `Client_Credentials` is not `client_credentials`.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
