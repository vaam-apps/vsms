The blocks of the document that are not providers: `deployment`, `security`,
`api` and `database`.

No field has a Rust-side default. The base file `config/application.yml` states
every value, so the defaults exist once, in YAML, and a test
(`tests/defaults.rs`) holds them to the defaults the clap flags had before
VSC-1. Every struct refuses unknown keys. `security` is the one block a process
may not have: the worker never loads it (`Binary::Worker`), so
`Config::security` is an error there.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
