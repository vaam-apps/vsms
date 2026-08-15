Turning a request into a policy-evaluable identity.

Every `@@allow` in the schema resolves against the four fields of the
`auth Principal` block — `sub`, `kind`, `role`, `appId`. `hasRole('admin')`
reads `role`; `auth().kind == "app"` reads `kind`; `appId == auth().appId`
reads `appId`. [`Principal::into_context`] is the single place those names
are produced, so a rename in the schema breaks exactly one function.

# #71: this is also where the correlation id enters `CoolContext`

`cratestack_core::CoolContext::request_id`/`with_request_id` has existed
since before this milestone (`cratestack-core`'s own doc: "Surfaces in
tracing spans and is recorded on audit events"), and every generated
CRUD/procedure route already logs `cratestack_request_id =
ctx.request_id().unwrap_or("")` (`cratestack-macros`'s
`list_result_log_tokens`/`dispatch_tail.rs`) — but nothing in this
deployment ever called `with_request_id`, so that field had been empty
on every single one of those log lines since the router first existed.
[`GatewayAuth::authenticate`] is the one place a [`CoolContext`] is
constructed per inbound HTTP request, so it is the natural, and only,
place to close that gap: honour an inbound `X-Request-Id` if the caller
sent one (so a client's own trace id survives into this system's logs
unchanged), otherwise mint a fresh one. Either way, every
`cratestack_*`-logged event for one HTTP request now shares one
`cratestack_request_id` — the correlation this crate's own custom
`message_id`-keyed events (`procedures.rs::send`) sit alongside, not a
replacement for them: a request id ties together everything logged
*within* this one process for *this* request; `message_id` is what
survives into `sms-worker`'s dispatch and the DLR ingestion path,
neither of which shares this process or this request. See
`docs/runbooks/alerting.md`'s own "Correlating a message end to end"
section for the worked example joining both.
