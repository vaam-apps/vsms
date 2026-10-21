import { describe, expect, it } from "vitest";
import { locate } from "./nav-groups";

describe("locate", () => {
  it("names the dashboard at the root with no section", () => {
    expect(locate("/")).toEqual({ page: "Dashboard" });
  });

  it("names a grouped page with its section", () => {
    expect(locate("/providers")).toEqual({ section: "Delivery", page: "Providers" });
  });

  it("matches a nested path to the page that lists it", () => {
    expect(locate("/messages/c8ae5b")).toEqual({ section: "Messaging", page: "Messages" });
  });

  it("does not let the root match every path", () => {
    expect(locate("/jobs")).toEqual({ section: "Operations", page: "Jobs" });
  });

  it("does not match a path that only shares a prefix", () => {
    expect(locate("/routesx")).toBeUndefined();
  });

  it("returns undefined for a path the navigation does not list", () => {
    expect(locate("/login")).toBeUndefined();
  });
});
