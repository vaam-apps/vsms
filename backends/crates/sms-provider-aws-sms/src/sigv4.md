AWS Signature Version 4, for exactly the one request shape this crate
sends: a `POST /` with a JSON body and no query string.

Hand-written rather than taken from `aws-sigv4`/`aws-sdk-*`. The SDK's
HTTP stack is the obvious source of `aws-lc-rs`, which this workspace keeps
out of its graph entirely (see `AGENTS.md`, "aws-lc-rs enters this tree from
authkestra alone"); the signing algorithm itself is about forty lines of
HMAC-SHA256 over the `hmac`/`sha2` crates the workspace already uses for
webhook signing.

A hand-written signer is only as good as its test vectors, so it is checked
against two independent references, never against itself:

- **AWS's published `get-vanilla` vector** from the `SigV4` test suite
  (`Signature=5fa00fa3…`).
- **botocore 1.43.116** signing the exact `SendTextMessage` request `submit`
  builds (`Signature=528f9f0c…`). The same harness reproduced the
  `get-vanilla` value first, which is what makes its second answer worth
  trusting.

Both fixtures use AWS's documentation credentials (`AKIDEXAMPLE`), which
authenticate nothing.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
