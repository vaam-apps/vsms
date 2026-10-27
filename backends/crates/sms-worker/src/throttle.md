Keyed "at most once per interval" for operator-facing log lines.

`hooks` has two warnings an operator must see and must not be drowned by: an
`hmac` endpoint that signs with an empty key (one line per endpoint, however many
events it receives) and a keyless worker's backlog of sealed deliveries it cannot
serve (one line per five minutes, however often the loop polls). Both are the
same question, "has this key been said recently?", so it is asked in one place.

[`Throttle::admit`] takes `now` as a parameter rather than reading the clock, so
the interval is proved with a fabricated one instead of a sleep. The map only
ever holds the keys that were admitted, which are the endpoints that actually
have the problem, so it needs no eviction.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
