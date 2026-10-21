import { describe, expect, it } from "vitest";
import { clientHref, internalPath, type LinkClick, type LinkTarget } from "./client-links";

const ORIGIN = "https://console.example";
const plain: LinkClick = {
  button: 0,
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  defaultPrevented: false,
};
const link = (href: string, overrides: Partial<LinkTarget> = {}): LinkTarget => ({
  href,
  target: "",
  hasDownload: false,
  ...overrides,
});

describe("clientHref", () => {
  it("routes a plain click on an internal link client-side", () => {
    expect(clientHref(plain, link(`${ORIGIN}/messages?state=failed#top`), ORIGIN)).toBe(
      "/messages?state=failed#top",
    );
  });

  it("accepts a relative href", () => {
    expect(clientHref(plain, link("/jobs"), ORIGIN)).toBe("/jobs");
  });

  it.each([["metaKey"], ["ctrlKey"], ["shiftKey"], ["altKey"]] as const)(
    "leaves a %s-click to the browser (new tab/window)",
    (key) => {
      expect(clientHref({ ...plain, [key]: true }, link("/jobs"), ORIGIN)).toBeNull();
    },
  );

  it("leaves a middle click to the browser", () => {
    expect(clientHref({ ...plain, button: 1 }, link("/jobs"), ORIGIN)).toBeNull();
  });

  it("leaves a click another handler already handled", () => {
    expect(clientHref({ ...plain, defaultPrevented: true }, link("/jobs"), ORIGIN)).toBeNull();
  });

  it("leaves target=_blank and downloads to the browser", () => {
    expect(clientHref(plain, link("/jobs", { target: "_blank" }), ORIGIN)).toBeNull();
    expect(clientHref(plain, link("/jobs", { hasDownload: true }), ORIGIN)).toBeNull();
    expect(clientHref(plain, link("/jobs", { target: "_self" }), ORIGIN)).toBe("/jobs");
  });
});

describe("internalPath", () => {
  it("refuses another origin", () => {
    expect(internalPath("https://evil.example/messages", ORIGIN)).toBeNull();
    expect(internalPath("//evil.example/messages", ORIGIN)).toBeNull();
  });

  it("never intercepts the auth routes, which must be real requests", () => {
    expect(internalPath("/api/auth/logout", ORIGIN)).toBeNull();
    expect(internalPath("/api", ORIGIN)).toBeNull();
    expect(internalPath("/apis", ORIGIN)).toBe("/apis");
  });

  it("refuses an href that is not a URL", () => {
    expect(internalPath("http://[", ORIGIN)).toBeNull();
  });
});
