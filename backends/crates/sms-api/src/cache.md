A time-to-live cache, used twice on `sendMessage`'s hot path: the
`clientId → App` lookup (§3.2: *"the token carries no `appId`... that's
on the hot path, so cache it — 60 seconds is short enough that retiring
a client takes effect promptly and long enough that the lookup never
matters"*) and the operator-prefix table (AGENTS.md: `previewMessage`
has reported `operator: unknown` since milestone 0 because nothing
queried `OperatorPrefixRule` yet — *"querying the DB means giving it a
cache and a refresh policy"*).

**Not** the `LISTEN`/`NOTIFY`-based opt-out-invalidation cache §11's
repository layout names this file for. That's a real R1 exception
(`LISTEN` is one of the three named ones) and a more sophisticated
mechanism than either cache here needs — a 60-second staleness window on
an `App` lookup is the behaviour §3.2 explicitly asks for, not a gap to
close. If a `LISTEN`-driven cache is built later, for opt-outs or
anything else, it can live in this same file; this type doesn't preclude
it.
