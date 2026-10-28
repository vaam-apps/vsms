`--config` / `SMS_CONFIG` and `--profile` / `SMS_PROFILE`, shared by the three
entry points that read the YAML.

`--profile` selects a **file**, never a code path: it must not be matched on
(`if profile == "prod"`) anywhere in the workspace. The default is `dev`, which
points at the local fakes and is never livemode, so forgetting the variable in
production cannot produce a deployment that sends real traffic.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
