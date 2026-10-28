`--config` / `SMS_CONFIG` and `--profile` / `SMS_PROFILE`, shared by the three
entry points that read the YAML.

`--profile` selects a **file**, never a code path: it must not be matched on
(`if profile == "prod"`) anywhere in the workspace. The default is `dev`, which
points at the local fakes and is never livemode, so forgetting the variable in
production cannot produce a deployment that sends real traffic. It is the only
profile that may name no file: any other profile with no
`application-<profile>.yml` is refused, naming the file. The price of the default
is that a release image run without `SMS_PROFILE` loads the dev overlay, and the
gateway then stops on the first thing in it a release build refuses
(`fake_sns_cert_base`); that message names the loaded profile and `SMS_PROFILE`.
vpay defaults to `sandbox` on the same reasoning, and nothing here argues for a
different default.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
