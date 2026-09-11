/**
 * `@vaam-apps/ui` — a dark-first React component library for operator
 * consoles: daisyUI for styling, Headless UI for behaviour, and a
 * parameterised status-glyph system for state machines.
 *
 * Import the token layer **once**, from the app's own global stylesheet,
 * after loading Tailwind and the daisyUI plugin:
 *
 * ```css
 * @import "tailwindcss";
 * @plugin "daisyui";
 * @import "@vaam-apps/ui/styles/theme.css";
 * @source "../node_modules/@vaam-apps/ui/dist";
 * ```
 *
 * The `@source` line is not optional: Tailwind v4 only generates the
 * utilities it can see used, and it does not look inside `node_modules`
 * by default. Omit it and every component renders unstyled — a silent
 * failure that is miserable to debug.
 */

// ---------------------------------------------------------------------------
// Data display — consistency is the deliverable, so these are components
// rather than ad hoc per-screen formatting.
// ---------------------------------------------------------------------------
export * from "./components/data/code";
export * from "./components/data/copy-button";
export * from "./components/data/detail-row";
export * from "./components/data/id-display";
export * from "./components/data/masked-value";
export * from "./components/data/money";
export * from "./components/data/phone-display";
export * from "./components/data/stat-tile";
export * from "./components/data/timestamp-display";
// ---------------------------------------------------------------------------
// Patterns — composed screen-level building blocks. Still domain-free,
// but opinionated about layout in a way the primitives are not.
// ---------------------------------------------------------------------------
export * from "./components/patterns/inline-banner";
export * from "./components/patterns/inline-empty-state";
export * from "./components/patterns/live-row";
export * from "./components/patterns/payload-inspector";
export * from "./components/patterns/route-skeleton";
export * from "./components/patterns/screen-layout";
export * from "./components/patterns/stale-write-banner";
export * from "./components/patterns/state-timeline";
// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------
export * from "./components/primitives/badge";
export * from "./components/primitives/button";
export * from "./components/primitives/calendar";
export * from "./components/primitives/card";
export * from "./components/primitives/checkbox";
export * from "./components/primitives/chip-select";
export * from "./components/primitives/command-menu";
export * from "./components/primitives/confirm-dialog";
export * from "./components/primitives/date-picker";
export * from "./components/primitives/dialog";
export * from "./components/primitives/drawer";
export * from "./components/primitives/dropdown-menu";
export * from "./components/primitives/form-field";
export * from "./components/primitives/inline-confirm";
export * from "./components/primitives/input";
export * from "./components/primitives/label";
export * from "./components/primitives/pagination";
export * from "./components/primitives/popover";
export * from "./components/primitives/progress";
export * from "./components/primitives/radio-group";
export * from "./components/primitives/select";
export * from "./components/primitives/separator";
export * from "./components/primitives/side-nav";
export * from "./components/primitives/skeleton";
export * from "./components/primitives/spinner";
export * from "./components/primitives/switch";
export * from "./components/primitives/table";
export * from "./components/primitives/tabs";
export * from "./components/primitives/textarea";
export * from "./components/primitives/toast";
export * from "./components/primitives/tooltip";
// ---------------------------------------------------------------------------
// Status — the most-reused surface in an operator console. Read
// `status-tokens.ts` first: the tables live in the consuming application,
// not here.
// ---------------------------------------------------------------------------
export * from "./components/status/state-chip";
export * from "./components/status/state-mark";
export * from "./components/status/status-pill";
export * from "./components/status/status-tokens";

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------
export { cn } from "./lib/cn";
export * from "./lib/money";
export { omitUndefined } from "./lib/omit-undefined";
