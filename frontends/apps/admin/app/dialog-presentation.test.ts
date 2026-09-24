// Every `Dialog` in this console, against AGENTS.md's rule for which
// presentation it takes ("Which `Dialog` presentation, and whose width"):
// `DialogFullScreen` for a form of three or more inputs, or a body that
// would scroll at 375×812; `DialogContent` for everything else. And, for
// each one, the wiring that has to survive the full-screen top bar: the
// confirm action still submits, still follows the pending state, and Cancel
// is a `DialogClose` the bar can hide.
//
// No DOM here — this workspace has no jsdom, and Headless UI portals its
// panel, so a static render sees nothing. Each view is a hook-free function
// component, so it is called directly and its element tree read. What that
// proves: the confirm button targets the `<form>` whose `onSubmit` calls the
// view's `onSubmit` (or calls it from its own `onClick`), and `DialogActions`
// is a direct part of the container, as the vaam-ui skill asks
// (`primitives-overlay.md`): inside a positioned wrapper of your own it never
// reaches the bar (measured: a `relative` wrapper left create-role's Create
// at y 524 in the body, not in the 64px bar). What it cannot prove is where
// anything is drawn:
// the bar's geometry, and that a click on the bar's button submits in a real
// browser, were measured with Playwright when this rule landed (PR #420),
// and the "would scroll at 375×812" half of the rule is a measurement too —
// recorded per dialog in `scrollsAt375` below rather than guessed at.
//
// Mutation-checked when written: dropping `form="create-app-form"` from the
// Create button, wrapping `DialogActions` in a `<div>`, dropping
// `disabled={isPending}`, putting an `onClick` back on a Cancel, turning a
// three-input form back into `DialogContent`, and adding a new dialog without
// a row here — in `app/`, in `components/`, behind an aliased import, or
// beside an existing one in a file that has a row — each fails this file.

import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import {
  Button,
  Dialog,
  DialogActions,
  DialogClose,
  DialogContent,
  DialogFullScreen,
  FormField,
} from "@vaam-apps/ui";
import { Fragment, isValidElement, type ReactElement, type ReactNode } from "react";
import type { FieldValues, UseFormReturn } from "react-hook-form";
import { describe, expect, it, vi } from "vitest";
import { CreateAppDialogView } from "./apps/components/create-app-dialog-view";
import type { JobListItem } from "./jobs/components/jobs-table";
import { RequeueConfirmDialog } from "./jobs/components/requeue-confirm-dialog";
import { RecordOptOutDialog } from "./opt-outs/components/record-opt-out-dialog";
import { RemoveConfirmDialog } from "./opt-outs/components/remove-confirm-dialog";
import { CreateSenderDialog } from "./sender-ids/components/create-sender-dialog";
import { CreateRoleDialogView } from "./users/components/create-role-dialog-view";
import { ProvisionUserDialogView } from "./users/components/provision-user-dialog-view";
import type { RoleRecord } from "./users/types";
import { CreateEndpointDialog } from "./webhooks/components/create-endpoint-dialog";
import { CreateEndpointFields } from "./webhooks/components/create-endpoint-fields";

type Props = Record<string, unknown> & { children?: ReactNode };
type El = ReactElement<Props>;

/** This console's own components that sit between a dialog and its form,
 * expanded in place (each is hook-free). Library parts are never expanded:
 * they are what the assertions look for. */
const EXPAND = new Set<unknown>([CreateEndpointFields]);

/** The element children of `node`, fragments and `EXPAND`ed components
 * flattened — the parts a container actually lays out. */
function parts(node: ReactNode): El[] {
  const out: El[] = [];
  const visit = (n: ReactNode) => {
    if (Array.isArray(n)) {
      for (const child of n) visit(child);
      return;
    }
    if (!isValidElement(n)) return;
    const el = n as El;
    if (el.type === Fragment) visit(el.props.children);
    else if (EXPAND.has(el.type)) visit((el.type as (p: Props) => ReactNode)(el.props));
    else out.push(el);
  };
  visit(node);
  return out;
}

/** Every element under `node`, depth first. */
function everything(node: ReactNode): El[] {
  return parts(node).flatMap((el) => [el, ...everything(el.props.children)]);
}

/** Enough of `UseFormReturn` for a view to render: `handleSubmit(fn)`
 * returns a handler that calls `fn` straight away, so calling the form's
 * `onSubmit` shows whether it reaches the view's own `onSubmit`. */
function fakeForm<T extends FieldValues>(): UseFormReturn<T> {
  const noop = () => {};
  return {
    handleSubmit: (fn: (values: T) => void) => (event?: { preventDefault?: () => void }) => {
      event?.preventDefault?.();
      fn({} as T);
    },
    register: (name: string) => ({ name, onChange: noop, onBlur: noop, ref: noop }),
    formState: { errors: {} },
    control: {},
  } as unknown as UseFormReturn<T>;
}

interface Case {
  /** Relative to `app/`. */
  file: string;
  /** Measured at 375×812, keyboard closed, as `DialogContent` (PR #420's
   * probe): the scrolling body's `scrollHeight` against its `clientHeight`.
   * None of the eight scrolls; the tallest, create-endpoint, is 594px of a
   * 690px cap with a mouse and 610px with a touch pointer (a coarse pointer
   * gets the library's wider padding and action spacing) — measure with
   * touch. Update this, measured, if a dialog's body grows. */
  scrollsAt375: boolean;
  /** Renders the view with `pending`, wiring its confirm to `confirm`. */
  render: (pending: boolean, confirm: () => void) => ReactNode;
}

const CASES: Case[] = [
  {
    file: "apps/components/create-app-dialog-view.tsx",
    scrollsAt375: false,
    render: (pending, confirm) =>
      CreateAppDialogView({
        open: true,
        onOpenChange: () => {},
        form: fakeForm(),
        onSubmit: confirm,
        isPending: pending,
        generalError: null,
      }),
  },
  {
    file: "users/components/create-role-dialog-view.tsx",
    scrollsAt375: false,
    render: (pending, confirm) =>
      CreateRoleDialogView({
        open: true,
        onOpenChange: () => {},
        form: fakeForm(),
        onSubmit: confirm,
        isPending: pending,
        generalError: null,
      }),
  },
  {
    file: "users/components/provision-user-dialog-view.tsx",
    scrollsAt375: false,
    render: (pending, confirm) =>
      ProvisionUserDialogView({
        open: true,
        roles: [{ key: "owner", label: "Owner" }] as unknown as RoleRecord[],
        form: fakeForm(),
        onSubmit: confirm,
        isPending: pending,
        isError: false,
        errorMessage: "",
        result: undefined,
        onDone: () => {},
      }),
  },
  {
    file: "sender-ids/components/create-sender-dialog.tsx",
    scrollsAt375: false,
    render: (pending, confirm) =>
      CreateSenderDialog({
        open: true,
        onOpenChange: () => {},
        form: fakeForm(),
        onSubmit: confirm,
        pending,
      }),
  },
  {
    file: "webhooks/components/create-endpoint-dialog.tsx",
    scrollsAt375: false,
    render: (pending, confirm) =>
      CreateEndpointDialog({
        open: true,
        onOpenChange: () => {},
        form: fakeForm(),
        eventTypes: ["message.delivered"],
        onEventTypesChange: () => {},
        onSubmit: confirm,
        pending,
      }),
  },
  {
    file: "opt-outs/components/record-opt-out-dialog.tsx",
    scrollsAt375: false,
    render: (pending, confirm) =>
      RecordOptOutDialog({
        open: true,
        onOpenChange: () => {},
        form: fakeForm(),
        onSubmit: confirm,
        isPending: pending,
      }),
  },
  {
    file: "jobs/components/requeue-confirm-dialog.tsx",
    scrollsAt375: false,
    render: (pending, confirm) =>
      RequeueConfirmDialog({
        job: { id: "job1", kind: "expire_stale", attempts: 5 } as unknown as JobListItem,
        pending,
        onOpenChange: () => {},
        onConfirm: confirm,
      }),
  },
  {
    file: "opt-outs/components/remove-confirm-dialog.tsx",
    scrollsAt375: false,
    render: (pending, confirm) =>
      RemoveConfirmDialog({ open: true, pending, onOpenChange: () => {}, onConfirm: confirm }),
  },
];

/** The container, its direct parts, and the confirm and Cancel actions. */
function anatomy(node: ReactNode) {
  const [root] = parts(node);
  expect(root?.type, "the view does not render a <Dialog> root").toBe(Dialog);
  const [container] = parts(root?.props.children);
  const direct = parts(container?.props.children);
  const actions = direct.find((el) => el.type === DialogActions);
  const inActions = parts(actions?.props.children);
  return {
    container,
    actions,
    confirms: inActions.filter((el) => el.type === Button),
    cancels: inActions.filter((el) => el.type === DialogClose),
    tree: everything(root?.props.children),
  };
}

const APP_DIR = fileURLToPath(new URL(".", import.meta.url));
/** The admin package root, so a dialog written in `components/` or `lib/`
 * beside `app/` is found too (its row's `file` then starts `../`). */
const ADMIN_DIR = fileURLToPath(new URL("..", import.meta.url));
/** A rendered container: `<DialogContent`, or `<ui.DialogFullScreen`. */
const CONTAINER_TAG = /<(?:\w+\.)?(?:DialogContent|DialogFullScreen)\b/g;
/** A named import of either, which catches the one the tag pattern cannot
 * see: an alias (`DialogFullScreen as Sheet`, rendered as `<Sheet>`). */
const CONTAINER_IMPORT =
  /import\s*\{[^}]*\b(?:DialogContent|DialogFullScreen)\b[^}]*\}\s*from\s*["']@vaam-apps\/ui["']/;

describe("every Dialog follows AGENTS.md's presentation rule, and its actions survive the bar", () => {
  it("covers every Dialog container in the console, one row each", () => {
    // One entry per container, so a second dialog added to a file that
    // already has a row needs a row of its own. Before this counted, and
    // walked the package root rather than `app/`, three new dialogs passed
    // unclassified: one in `components/`, one behind an aliased import, and
    // a second one appended to `remove-confirm-dialog.tsx`.
    const found: string[] = [];
    const walk = (dir: string) => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        if (entry.name === "node_modules" || entry.name.startsWith(".")) continue;
        const path = join(dir, entry.name);
        if (entry.isDirectory()) walk(path);
        else if (entry.name.endsWith(".tsx")) {
          const source = readFileSync(path, "utf8");
          const tags = source.match(CONTAINER_TAG)?.length ?? 0;
          const containers = Math.max(tags, CONTAINER_IMPORT.test(source) ? 1 : 0);
          for (let i = 0; i < containers; i++) found.push(relative(APP_DIR, path));
        }
      }
    };
    walk(ADMIN_DIR);
    expect(
      found.sort(),
      "a dialog without a row in CASES — add one, and classify it by the rule",
    ).toEqual(CASES.map((c) => c.file).sort());
  });

  for (const c of CASES) {
    describe(c.file, () => {
      it("takes the presentation the rule gives it", () => {
        const { container, tree } = anatomy(c.render(false, () => {}));
        // A labelled control is a `FormField`, or a `<fieldset>` for a group
        // no single label can name (create-endpoint's event types).
        const inputs = tree.filter((el) => el.type === FormField || el.type === "fieldset").length;
        const expected = inputs >= 3 || c.scrollsAt375 ? DialogFullScreen : DialogContent;
        expect(
          container?.type,
          `${inputs} inputs, scrolls at 375×812: ${c.scrollsAt375} — expected ${expected.name}`,
        ).toBe(expected);
        // The library owns the width: no per-dialog `max-w-*`.
        expect(container?.props.className, "a per-dialog width").toBeUndefined();
      });

      it("keeps DialogActions a direct part, so the full-screen bar can take it", () => {
        const { actions } = anatomy(c.render(false, () => {}));
        expect(actions, "DialogActions is not a direct part of the container").toBeDefined();
      });

      it("confirms through the action in DialogActions, and only once per click", () => {
        const confirm = vi.fn();
        const { confirms, tree } = anatomy(c.render(false, confirm));
        expect(confirms, "not exactly one confirming Button in DialogActions").toHaveLength(1);
        const [button] = confirms as [El];
        const event = { preventDefault: vi.fn() };
        if (button.props.type === "submit") {
          // Outside the <form> (DialogActions is a sibling of it), a submit
          // button submits only the form its `form` attribute names.
          const form = tree.find((el) => el.type === "form" && el.props.id === button.props.form);
          expect(form, `no <form id=${String(button.props.form)}> for the submit`).toBeDefined();
          expect(button.props.onClick, "a submit that also has an onClick").toBeUndefined();
          const onSubmit = form?.props.onSubmit as ((e: unknown) => void) | undefined;
          onSubmit?.(event);
        } else {
          (button.props.onClick as (e: unknown) => void)(event);
        }
        expect(confirm).toHaveBeenCalledTimes(1);
      });

      it("disables the confirm while pending, and only then", () => {
        const idle = anatomy(c.render(false, () => {})).confirms[0];
        const busy = anatomy(c.render(true, () => {})).confirms[0];
        expect(idle?.props.disabled, "disabled while idle").toBeFalsy();
        expect(busy?.props.disabled, "not disabled while pending").toBe(true);
      });

      it("dismisses with one DialogClose as={Button} and no onClick of its own", () => {
        const { cancels } = anatomy(c.render(false, () => {}));
        expect(cancels, "not exactly one DialogClose in DialogActions").toHaveLength(1);
        const [cancel] = cancels as [El];
        expect(cancel.props.as, "Cancel is not rendered as a Button").toBe(Button);
        // `DialogClose` already calls `onOpenChange(false)`; an `onClick`
        // that closes too runs the handler twice (AGENTS.md, 0.3.0 section).
        expect(cancel.props.onClick, "Cancel closes twice").toBeUndefined();
      });
    });
  }
});
