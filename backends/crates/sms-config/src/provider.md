The `providers` list: one entry per SMS provider this deployment runs.

An entry's presence is what configures the provider, so an incomplete one is an
error naming the missing key, not a provider quietly left out. That replaces the
all-or-none checks `sms-gateway` and `sms-worker` each carried for their flags.
An absent entry is a provider the deployment does not run, and a deployment with
none cannot serve (the gateway has nothing to parse delivery receipts with) or
dispatch (the worker has nothing to send through).

| `code`      | `host.url`                           | `settings`                                                                                                                                                                                                                                                     | `credentials`                        |
| ----------- | ------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------ |
| `orange_cm` | defaults to `https://api.orange.com` | `sender_number`                                                                                                                                                                                                                                                | `client_id`, `client_secret`         |
| `mtn_cm`    | defaults to `https://api.mtn.com`    | `service_code`, `tps_ceiling`, `cost_per_segment_xaf`, `sender_id`?, `supports_alphanumeric_sender`                                                                                                                                                            | `client_id`, `client_secret`         |
| `aws_sms`   | the endpoint override, optional      | sending: `region`, `configuration_set`, `tps_ceiling`, `cost_per_segment_xaf`, and optionally `origination_identity`, `message_type`, `supports_alphanumeric_sender`, `time_to_live_seconds`; receiving: `events_topic_arn`; demo builds: `fake_sns_cert_base` | `access_key_id`, `secret_access_key` |

`tps_ceiling` and `cost_per_segment_xaf` have no default on purpose: they are
commercial terms of a contract, and an invented one either throttles a paid
contract or misprices every message. `cost_per_segment_xaf` is money; write it
as a quoted string (`"15.00"`) so it never passes through a float.

Quote anything that looks like a number and is text: `sender_number: "+2370000"`
and `service_code: "131"` are strings, and the unquoted spellings are YAML
integers, which the typed model refuses.

`aws_sms` has two independent halves, as the two binaries do: the worker sends
with the six values AWS has no safe default for (all-or-none), and the gateway
only needs `events_topic_arn`. An entry with neither is refused.

The conversions to the adapters' own config types
([`OrangeCm::to_provider_config`] and its siblings) live here so the gateway
and the worker cannot build a provider differently. The connect and request
timeouts are the ones both binaries always used: 10 and 30 seconds.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
