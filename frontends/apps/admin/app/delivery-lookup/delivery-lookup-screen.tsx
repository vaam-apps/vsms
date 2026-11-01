"use client";

// The Delivery lookup screen (#472): "did this message arrive?" for a support
// agent, who may not read the message itself. One key per lookup (a message
// id, a client reference with its app, or a whole phone number), a window for
// the last two, and the answer: a masked recipient, a state, the lifecycle
// timestamps and the newest receipt.
//
// R6: this file is the smart component. Markup and classes live in
// `components/*`; the rules (what is valid, the arguments, which error says
// what) live in `lookup-query.ts`, `lookup-error.ts` and `lookup-labels.ts`,
// each with a test.
//
// # The lookup is a mutation
//
// It reads, but every call that reaches the database writes an audit row and a
// lookup by number or reference spends the hourly budget. A query would be
// retried on a 5xx and refetched on focus. See `@vsms/api`'s
// `routers/delivery-status.ts`.
//
// # What is, and is not, in the URL
//
// Only `by` (which key) and `detail` (a message id the drawer shows). Never
// what was typed: an address is kept in history and in server logs, and a
// phone number is personal data. The number stays in its input and nowhere
// else: not in the results, the empty state, the drawer, the summary line a
// screen reader announces, or an error. It is deliberately not cleared after a
// lookup, because widening the window and asking again is the usual next step
// and retyping a number is how a budgeted lookup gets wasted on a typo.
//
// # Why the navigation is not hidden from roles that cannot use this
//
// The console cannot know the person's permissions: the schema has no
// who-am-I procedure. The page says up front who may use it and renders the
// 403 plainly. The same gap is recorded for Opt-outs (AGENTS.md).

import { zodResolver } from "@hookform/resolvers/zod";
import { QuickDetailDrawer, ScreenStack } from "@vaam-apps/ui";
import { trpc } from "@vsms/hooks";
import { parseAsString, useQueryStates } from "nuqs";
import { useRef } from "react";
import { useForm, useWatch } from "react-hook-form";
import { Caption } from "@/components/caption";
import { PageHeader } from "@/components/page-header";
import { SectionHeader } from "@/components/section-header";
import { LookupDetailFields } from "./components/lookup-detail-fields";
import { LookupForm } from "./components/lookup-form";
import { LookupResults } from "./components/lookup-results";
import { focusChoice } from "./focus-choice";
import { describeLookupError, typedValues } from "./lookup-error";
import { appNameIndex } from "./lookup-labels";
import {
  activePresetDays,
  defaultValues,
  earliestFrom,
  isLookupReady,
  type LookupFormValues,
  type LookupKind,
  lookupSchema,
  parseKind,
  presetWindow,
  toLookupArgs,
} from "./lookup-query";
import { ignoreHeldEnter, runOnce } from "./submit-guard";

export function DeliveryLookupScreen() {
  const [urlState, setUrlState] = useQueryStates(
    { by: parseAsString, detail: parseAsString },
    { history: "replace" },
  );
  const kind = parseKind(urlState.by);

  const form = useForm<LookupFormValues>({
    // The kind and "today" are read when a check runs, not when the form is
    // made, so a page left open across midnight validates against the real day.
    resolver: (values, context, options) =>
      zodResolver(lookupSchema(kind, new Date()))(values, context, options),
    defaultValues: defaultValues(new Date()),
    mode: "onTouched",
  });
  const values = useWatch({ control: form.control });
  const now = new Date();
  const current: LookupFormValues = { ...defaultValues(now), ...values };

  const appsQuery = trpc.apps.list.useQuery();
  const apps = appsQuery.data ?? [];

  // A lookup is in flight from the moment it is handed to `mutate` until it
  // settles. `isPending` says so too, but only after the next render, and two
  // submits can land before it (a held Enter, a double tap): each would spend
  // budget. A ref changes at once. See `submit-guard.ts`.
  const inFlight = useRef(false);
  const appGroupRef = useRef<HTMLDivElement>(null);

  // An error's text is kept free of what was *sent* (`variables`), not of the
  // form as it stands, which the person may have edited since.
  const lookup = trpc.deliveryStatus.lookup.useMutation({
    onError: (error, sent) => {
      const view = describeLookupError(
        { code: error.data?.code, message: error.message },
        typedValues(sent),
      );
      if (view.kind === "field") {
        form.setError(view.field, { message: view.text }, { shouldFocus: true });
        // The app picker is not a registered input, so `shouldFocus` cannot
        // reach it; its container is only there when apps are listed.
        if (view.field === "appId") focusChoice(appGroupRef.current);
      }
    },
    onSettled: () => {
      inFlight.current = false;
    },
  });

  const failure =
    lookup.error === null
      ? null
      : describeLookupError(
          { code: lookup.error.data?.code, message: lookup.error.message },
          typedValues(lookup.variables),
        );

  const detailRow =
    urlState.detail === null || lookup.data === undefined
      ? null
      : (lookup.data.statuses.find((row) => row.messageId === urlState.detail) ?? null);
  const appNames = appNameIndex(apps);

  function changeKind(next: LookupKind) {
    form.clearErrors();
    lookup.reset();
    void setUrlState({ by: next, detail: null });
  }

  function choosePreset(days: number) {
    const window = presetWindow(days, new Date());
    form.setValue("from", window.from, { shouldDirty: true, shouldValidate: true });
    form.setValue("to", window.to, { shouldDirty: true, shouldValidate: true });
  }

  function chooseApp(appId: string) {
    form.setValue("appId", appId, { shouldDirty: true, shouldValidate: true });
  }

  const submit = form.handleSubmit((submitted) => {
    runOnce(inFlight, () => {
      void setUrlState({ detail: null });
      lookup.mutate(toLookupArgs(submitted, kind));
    });
  });

  return (
    <ScreenStack>
      <PageHeader
        title="Delivery lookup"
        description="Did this message arrive? Find it by id, client reference or phone number, without reading what it said."
      />

      <LookupForm
        kind={kind}
        onKindChange={changeKind}
        register={form.register}
        errors={form.formState.errors}
        apps={apps}
        appsLoading={appsQuery.isLoading}
        appId={current.appId}
        onAppChange={chooseApp}
        presetDays={activePresetDays(current.from, current.to, now)}
        onPreset={choosePreset}
        earliestFrom={earliestFrom(kind, now)}
        canSubmit={isLookupReady(current, kind, now)}
        isPending={lookup.isPending}
        bannerText={failure?.kind === "banner" ? failure.text : null}
        appGroupRef={appGroupRef}
        onSubmit={submit}
        onKeyDown={ignoreHeldEnter}
      />

      {(lookup.isPending || lookup.isSuccess) && (
        <>
          <SectionHeader title="Results" />
          <LookupResults
            result={lookup.data}
            isPending={lookup.isPending}
            kind={kind}
            appNames={appNames}
            onOpen={(messageId) => void setUrlState({ detail: messageId })}
          />
          <Caption>
            Recipients are shown masked. A message that has been purged shows no recipient at all.
          </Caption>
        </>
      )}

      <QuickDetailDrawer
        open={detailRow !== null}
        onOpenChange={(open) => !open && void setUrlState({ detail: null })}
        title="Delivery status"
        description={detailRow !== null ? `Message ${detailRow.messageId}` : undefined}
      >
        {detailRow !== null && (
          <LookupDetailFields status={detailRow} appName={appNames[detailRow.appId]} />
        )}
      </QuickDetailDrawer>
    </ScreenStack>
  );
}
