What the fake decides to do with one submit call: how to answer the HTTP
request, and whether/when/what to POST back as a DLR. Two policies pick
that decision — see [`FaultPolicy`]'s own doc for the CI-determinism
reasoning behind having exactly two, not a spectrum.

Connection-level nastiness (RST mid-response, refused connections,
byte-dribble) is deliberately not modelled here — out of scope for this
PR, noted as future work rather than half-done. Everything below answers
with a real, well-formed (if sometimes broken-on-purpose) HTTP response;
[`sms_provider_orange_cm`]'s own unit tests already cover the
connect-vs-post-connect transport-error distinction directly against
real refused/slow sockets, so this crate doesn't need to re-prove that
part.
