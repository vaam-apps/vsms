// What the Delivery lookup's components put on the page, rendered: the masked
// recipient, the purged marker, the truncated footer, the summary line, and the
// form's number input. `renderToStaticMarkup(createElement(...))` in a `.test.ts`
// file, as `jobs/components/job-detail-fields.test.ts` does it, because this
// app's tsconfig sets `"jsx": "preserve"` and no DOM is available.

import * as React from "react";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { UseFormRegister } from "react-hook-form";
import { describe, expect, it } from "vitest";
import type { LookupFormValues } from "../lookup-query";
import { LookupDetailFields } from "./lookup-detail-fields";
import { LookupForm, type LookupFormProps } from "./lookup-form";
import { type LookupResult, LookupResults, type LookupStatus } from "./lookup-results";

(globalThis as unknown as { React: typeof React }).React = React;

const TYPED_NUMBER = "+237699887766";

const STATUS: LookupStatus = {
  messageId: "cmsg0000000000000000001",
  appId: "capp0000000000000000001",
  recipient: "+237*******56",
  operator: "orange",
  senderIdValue: "VAAM",
  class: "otp",
  encoding: "gsm7",
  segments: 1,
  state: "delivered",
  attempts: 1,
  acceptedAt: "2026-10-30T10:00:00.000Z",
  submittedAt: "2026-10-30T10:00:01.000Z",
  finalizedAt: "2026-10-30T10:00:05.000Z",
  expiresAt: "2026-10-31T10:00:00.000Z",
  purged: false,
  lastReceipt: {
    id: "crcpt0000000000000000001",
    providerId: "cprov0000000000000000001",
    outcome: "in_flight",
    rawStatus: "DeliveredToNetwork",
    networkCode: "orange",
    receivedAt: "2026-10-30T10:00:03.000Z",
  },
};

function results(
  result: LookupResult | undefined,
  over: Partial<{ isPending: boolean; kind: "id" | "ref" | "number" }> = {},
): string {
  return renderToStaticMarkup(
    createElement(LookupResults, {
      result,
      isPending: over.isPending ?? false,
      kind: over.kind ?? "number",
      appNames: { [STATUS.appId]: "Mobile money" },
      onOpen: () => undefined,
    }),
  );
}

describe("the results", () => {
  it("shows a masked recipient, the state in words, and the app's name", () => {
    const html = results({ statuses: [STATUS], truncated: false });
    expect(html).toContain("+237*******56");
    expect(html).toContain("Delivered");
    expect(html).toContain("Mobile money");
    expect(html).toContain("1 message found");
  });

  it("a purged row says Purged and carries no recipient", () => {
    const { recipient: _recipient, ...rest } = STATUS;
    const html = results({ statuses: [{ ...rest, purged: true }], truncated: false });
    expect(html).toContain("Purged");
    expect(html).not.toContain("+237");
  });

  it("a row with no recipient field reads as purged even if the flag were false", () => {
    const { recipient: _recipient, ...rest } = STATUS;
    expect(results({ statuses: [rest], truncated: false })).toContain("Purged");
  });

  it("the truncated footer appears only when the server said so", () => {
    expect(results({ statuses: [STATUS], truncated: true })).toContain(
      "Showing the newest 1. More messages matched: narrow the window.",
    );
    expect(results({ statuses: [STATUS], truncated: false })).not.toContain("Showing the newest");
  });

  it("an empty answer says no message matched and what that can mean for the key", () => {
    const html = results({ statuses: [], truncated: false }, { kind: "number" });
    expect(html).toContain("No message matched");
    expect(html).toContain("pepper");
  });

  it("while pending it says so, marks the region busy and shows no stale rows", () => {
    const html = results({ statuses: [STATUS], truncated: false }, { isPending: true });
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain("Looking up…");
    expect(html).not.toContain("+237*******56");
  });

  it("the summary is announced politely and holds no input", () => {
    const html = results({
      statuses: [STATUS, { ...STATUS, messageId: "cmsg2" }],
      truncated: false,
    });
    expect(html).toContain('aria-live="polite"');
    expect(html).toContain("2 messages found");
    expect(html).not.toContain(TYPED_NUMBER);
  });

  it("a row is a button named by its message id", () => {
    const html = results({ statuses: [STATUS], truncated: false });
    expect(html).toContain(`aria-label="View details for message ${STATUS.messageId}"`);
    expect(html).toContain('role="button"');
  });
});

describe("the detail fields", () => {
  const detail = (status: LookupStatus) =>
    renderToStaticMarkup(createElement(LookupDetailFields, { status, appName: undefined }));

  it("shows every lifecycle field and the receipt, in_flight as progress", () => {
    const html = detail(STATUS);
    for (const label of [
      "Message id",
      "State",
      "Recipient",
      "Accepted",
      "Submitted",
      "Finalized",
      "Expires",
      "Last receipt",
    ]) {
      expect(html).toContain(label);
    }
    expect(html).toContain("In flight (progress, not an outcome)");
    expect(html).toContain("DeliveredToNetwork");
  });

  it("says None yet for a message with no receipt, and a dash for an unset instant", () => {
    const { lastReceipt: _r, finalizedAt: _f, ...rest } = STATUS;
    const html = detail(rest);
    expect(html).toContain("None yet");
    expect(html).toContain("—");
  });
});

const register = ((name: string) => ({
  name,
  onChange: async () => undefined,
  onBlur: async () => undefined,
  ref: () => undefined,
})) as unknown as UseFormRegister<LookupFormValues>;

function form(over: Partial<LookupFormProps> = {}): string {
  return renderToStaticMarkup(
    createElement(LookupForm, {
      kind: "number",
      onKindChange: () => undefined,
      register,
      errors: {},
      apps: [],
      appsLoading: false,
      appId: "",
      onAppChange: () => undefined,
      presetDays: 7,
      onPreset: () => undefined,
      earliestFrom: "2026-08-02",
      canSubmit: true,
      isPending: false,
      bannerText: null,
      onSubmit: () => undefined,
      ...over,
    }),
  );
}

describe("the form", () => {
  it("keeps the number input out of the browser's memory, and names it", () => {
    const html = form();
    const input = /<input[^>]*id="lookup-msisdn"[^>]*>/.exec(html)?.[0] ?? "";
    // HTML attribute names are case-insensitive; the renderer keeps React's casing.
    expect(input).toMatch(/autocomplete="off"/i);
    expect(input).toMatch(/inputmode="tel"/i);
    expect(input).toMatch(/spellcheck="false"/i);
    expect(html).toMatch(/<label[^>]*for="lookup-msisdn"[^>]*>Phone number/);
  });

  it("has the whole-number input only for the number key", () => {
    expect(form({ kind: "id" })).not.toContain("lookup-msisdn");
    expect(form({ kind: "ref" })).not.toContain("lookup-msisdn");
  });

  it("a lookup by id has no window controls", () => {
    const html = form({ kind: "id" });
    expect(html).toContain("lookup-message-id");
    expect(html).not.toContain("lookup-from");
    expect(html).not.toContain("Last 7 days");
  });

  it("a number's From input carries the earliest date the server accepts", () => {
    expect(/<input[^>]*id="lookup-from"[^>]*>/.exec(form())?.[0]).toContain('min="2026-08-02"');
  });

  it("the submit button is disabled until the form is ready, and while pending", () => {
    expect(form({ canSubmit: false })).toMatch(
      /<button[^>]*disabled[^>]*type="submit"|<button[^>]*type="submit"[^>]*disabled/,
    );
    const pending = form({ isPending: true });
    expect(pending).toMatch(/<button[^>]*disabled/);
    expect(pending).toContain("Looking up…");
    expect(form()).not.toMatch(/<button[^>]*type="submit"[^>]*disabled/);
  });

  it("a reference offers the apps as a radio group when it may list them", () => {
    const html = form({ kind: "ref", apps: [{ id: STATUS.appId, name: "Mobile money" }] });
    expect(html).toContain('role="radiogroup"');
    expect(html).toContain("Mobile money");
    expect(html).not.toContain("lookup-app-id");
  });

  it("a reference asks for the app's id when the role cannot list apps", () => {
    const html = form({ kind: "ref", apps: [] });
    expect(html).toContain("lookup-app-id");
    expect(html).toContain("Your role cannot list apps");
  });

  it("shows a failure that belongs to no field as an alert", () => {
    const html = form({ bannerText: "The lookup did not complete." });
    expect(html).toContain('role="alert"');
    expect(html).toContain("The lookup did not complete.");
  });

  it("states the permission and the budget up front", () => {
    const html = form();
    expect(html).toContain("delivery:read");
    expect(html).toContain("60 an hour");
  });

  it("never prints a typed number: nothing in the form is given one", () => {
    expect(form()).not.toContain(TYPED_NUMBER);
  });
});
