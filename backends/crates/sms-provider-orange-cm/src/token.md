`OAuth2` `client_credentials` token acquisition and caching.

§6.2: *"TTL 3600s. Cache and refresh at 80% of life; do not fetch a
token per message."* At Orange's own 5 TPS ceiling a per-message fetch
would mean the OAuth endpoint sees the same load as the SMS endpoint —
this is not an optimisation, it is the difference between one dependency
on `oauth/v3/token` and the whole submit path having two.
