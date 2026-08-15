Conventions that more than one crate has to agree on, starting with the
one the schema forced on us.

# Sentinel-delimited multi-value columns

No model in `schema.cstack` declares a list field, because `String[]` and
`Int[]` panic the server macro — the parser accepts them and the migration
emitter writes `TEXT[]`, but `include_server_schema!` dies with
`unsupported SQLx value type for this slice` (§2.0). So every multi-value
column is a space-delimited `String`.

The part that is easy to get wrong, and the reason this lives in one place:
the values are stored **with leading and trailing separators**.

```text
" sms:send sms:read "      not      "sms:send sms:read"
```

Those sentinel spaces are what make a membership test safe. A stored
`"sms:send sms:read"` matched with `.contains("sms:send")` also matches a
hypothetical `sms:sendall`; `" sms:send sms:read "` matched with
`.contains(" sms:send ")` cannot. Since the filter language has no
`not_in`, `between` or `ilike` (§2.0), `.contains(...)` is the membership
test available, and it is only correct against the sentinel form.

An empty collection is a single space, not the empty string — `""` has no
separators, so it would false-match nothing but also could not be extended
without special-casing. The `' '` default is set per column in
`0002_bootstrap`.

For `scopes`, `grantTypes` and `redirectUris` this is barely a workaround:
OAuth transmits all three as space-delimited strings anyway, so the column
and the wire format finally agree.
