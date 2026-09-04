Classifies a *transport*-level failure from `.send()` itself — the network
call never produced an HTTP response at all, as distinct from a well-formed
response carrying a non-`2xx` status
([`crate::submit_status::classify_common_submit_status`]'s job, plus
whatever an adapter's own `classify_submit_error` layers in front of it).

Ported verbatim from `sms-provider-orange-cm`'s original
`classify_transport_error` (#33, sharpened by #119) — the predicate order,
the three branches, and every word of the non-parameterised message text
are unchanged. Only the provider noun moved from being hardcoded per crate
to a `provider: &str` argument; see this function's own doc below for
exactly where that noun lands and why nothing else about the wording
differs from what `sms-provider-orange-cm`/`sms-provider-mtn` each said
before this crate existed.

The connect-vs-post-connect distinction is the whole point — a submit that
times out after the request was already sent must never be resubmitted,
since the provider may have already sent the SMS.
[`reqwest::Error::is_connect`] is `true` only for a failure establishing
the connection itself — DNS resolution, TCP handshake, TLS handshake,
including a *connect-phase* timeout. At that point the adapter has not
written a single byte of the request onto a socket the provider controls,
so retrying — this provider or the next — is exactly as safe as it always
was. Checked first, and returns early, so nothing below it re-examines a
connect-phase error.

[`reqwest::Error::is_timeout`] is `true` for a timeout at *either* phase;
by construction (`is_connect` already handled and returned above) reaching
this check with `is_timeout() == true` means the connection was already
established and the timeout fired while writing the request body or
waiting on the provider's response. Both adapters' own request bodies are
fully buffered before `.send()` starts writing them (`req.json(&body)`), so
by the time any of this fires, the socket write of that buffer is either
complete or in progress — the provider's server may already have the full
request and be acting on it. Genuinely unknown, not safe to retry.

[`reqwest::Error::is_body`] covers the same "past the connect phase"
territory from a different failure shape — the connection was reset or
closed while a body (ours going out, or the provider's coming back) was
being transferred, rather than the client's own timeout firing. Same
reasoning applies, so it's grouped with the timeout case rather than
falling through to the conservative default.

Anything else — a `Builder`/`Request`-kind error, a bad `Url`, a
redirect-policy violation — never got as far as writing to a socket at
all, so it stays `Unavailable`, matching this function's behaviour before
this crate existed.

Getting this backwards either destroys legitimate failover (a real
connect refusal marked `Indeterminate` would strand every submission to a
genuinely dead provider in `uncertain` instead of failing over) or
reintroduces the duplicate-SMS bug #119 exists to prevent (a post-connect
timeout marked `Unavailable` gets resubmitted, possibly a second time to
the same handset). The module-level guard-failure proof in this crate's
own tests, and the equivalent proof each adapter keeps at its own
integration point, both exist to catch exactly that regression — see
`AGENTS.md`'s "Cleanup: one transport classifier for every HTTP adapter"
section for the actual sabotage-and-restore run.
