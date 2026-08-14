// #55: `packEventTypes`/`unpackEventTypes` have to agree byte-for-byte with
// `sms_core::pack`/`unpack` (`backends/crates/sms-core/src/lib.rs`) — this is the
// wire format `dlr.rs`'s own subscriber match
// (`webhook_endpoint::eventTypes().contains(sms_core::needle(event_type))`)
// depends on. A drift here wouldn't fail loudly; it would mean an operator's
// event-type selection silently stops matching, or silently matches
// everything (an empty column read as "no filter" rather than "no events").

import { describe, expect, it } from "vitest";
import { packEventTypes, unpackEventTypes } from "./webhooks";

describe("packEventTypes", () => {
  it("wraps a single value in sentinel spaces", () => {
    expect(packEventTypes(["message.delivered"])).toBe(" message.delivered ");
  });

  it("joins multiple values with exactly one space, sentinels on both ends", () => {
    expect(packEventTypes(["message.accepted", "message.delivered"])).toBe(
      " message.accepted message.delivered ",
    );
  });

  /**
   * The actual trap this file exists to catch — see `webhooks.ts`'s own
   * doc comment on [`packEventTypes`]. `sms_core::pack(Vec::<String>::new())
   * == EMPTY == " "`, one space, not two. A naive `` ` ${arr.join(" ")} ` ``
   * implementation produces `"  "` for an empty array instead — this
   * assertion is what would have caught that regression before it shipped.
   */
  it("packs an empty selection to exactly one space, not two", () => {
    expect(packEventTypes([])).toBe(" ");
    expect(packEventTypes([]).length).toBe(1);
  });
});

describe("unpackEventTypes", () => {
  it("round-trips through packEventTypes", () => {
    const types = ["message.accepted", "message.delivered", "message.failed"] as const;
    expect(unpackEventTypes(packEventTypes(types))).toEqual(types);
  });

  it("unpacks the empty-selection sentinel to an empty array", () => {
    expect(unpackEventTypes(" ")).toEqual([]);
  });

  it("is tolerant of repeated internal whitespace, matching sms_core::unpack's split_whitespace", () => {
    expect(unpackEventTypes("  message.accepted   message.delivered  ")).toEqual([
      "message.accepted",
      "message.delivered",
    ]);
  });
});
