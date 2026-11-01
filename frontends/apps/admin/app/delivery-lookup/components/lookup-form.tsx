// Dumb component (R6): the lookup's form. One question at a time: which key
// names the message, then that key's field, then (for a reference or a number)
// the window. Every value comes in as a prop or through the `register` the
// screen's react-hook-form hands over; nothing is fetched here.
//
// The choice between keys is a radio group, shown whole, and the windows are
// filter chips: nothing hides its options behind a trigger.
//
// The number input keeps itself out of the browser's own memory
// (`autoComplete="off"`, no spellcheck, no capitalisation or correction): a
// phone number typed here is personal data, and the page never copies it
// anywhere else (see `delivery-lookup-screen.tsx`).

import {
  Button,
  FieldError,
  FormField,
  groupLabelId,
  InlineBanner,
  Input,
  RadioGroup,
  Skeleton,
} from "@vaam-apps/ui";
import type { FormEventHandler, KeyboardEventHandler, RefObject } from "react";
import type { FieldErrors, UseFormRegister } from "react-hook-form";
import { FilterChip } from "@/components/filter-chip";
import { BUDGET_NOTE, KIND_OPTIONS, WINDOW_HINT } from "../lookup-labels";
import { type LookupFormValues, type LookupKind, WINDOW_PRESETS } from "../lookup-query";

export interface LookupFormApp {
  id: string;
  name: string;
}

export interface LookupFormProps {
  kind: LookupKind;
  onKindChange: (kind: LookupKind) => void;
  register: UseFormRegister<LookupFormValues>;
  errors: FieldErrors<LookupFormValues>;
  /** The apps the person may list; empty for a role that may not. */
  apps: readonly LookupFormApp[];
  appsLoading: boolean;
  appId: string;
  onAppChange: (appId: string) => void;
  /** The preset whose window is the current one, if any. */
  presetDays: number | null;
  onPreset: (days: number) => void;
  /** The earliest From the kind accepts. */
  earliestFrom: string | undefined;
  canSubmit: boolean;
  isPending: boolean;
  /** A failure that belongs to no field. */
  bannerText: string | null;
  /** Where the app picker's radios live, so the screen can focus them. */
  appGroupRef: RefObject<HTMLDivElement | null>;
  onSubmit: FormEventHandler<HTMLFormElement>;
  onKeyDown: KeyboardEventHandler<HTMLFormElement>;
}

export function LookupForm(props: LookupFormProps) {
  const { kind, register, errors } = props;
  return (
    <section className="flex flex-col gap-4 rounded-[20px] bg-surface-container-low p-4 sm:p-5">
      <p className="max-w-3xl text-body-small text-muted-foreground">{BUDGET_NOTE}</p>

      <form
        aria-label="Delivery lookup"
        noValidate
        className="flex flex-col gap-4"
        onSubmit={props.onSubmit}
        onKeyDown={props.onKeyDown}
      >
        <FormField label="Look up by" htmlFor="lookup-kind" control="group">
          <RadioGroup
            aria-labelledby={groupLabelId("lookup-kind")}
            value={kind}
            onValueChange={props.onKindChange}
            options={KIND_OPTIONS}
            disabled={props.isPending}
          />
        </FormField>

        {kind === "id" && (
          <FormField
            label="Message id"
            htmlFor="lookup-message-id"
            error={errors.messageId?.message}
          >
            <Input
              id="lookup-message-id"
              autoComplete="off"
              spellCheck={false}
              placeholder="Paste the id"
              className="font-mono"
              {...register("messageId")}
            />
          </FormField>
        )}

        {kind === "ref" && (
          <>
            <FormField
              label="Client reference"
              htmlFor="lookup-client-ref"
              error={errors.clientRef?.message}
              hint="Matched exactly, within the app below."
            >
              <Input
                id="lookup-client-ref"
                autoComplete="off"
                spellCheck={false}
                {...register("clientRef")}
              />
            </FormField>
            {props.appsLoading && <Skeleton className="h-12 w-full" />}
            {!props.appsLoading && props.apps.length > 0 && (
              <FormField label="App" htmlFor="lookup-app" control="group">
                <div ref={props.appGroupRef}>
                  <RadioGroup
                    aria-labelledby={groupLabelId("lookup-app")}
                    value={props.appId === "" ? undefined : props.appId}
                    onValueChange={props.onAppChange}
                    options={props.apps.map((app) => ({ value: app.id, label: app.name }))}
                  />
                </div>
                {errors.appId?.message !== undefined && (
                  <FieldError>{errors.appId.message}</FieldError>
                )}
              </FormField>
            )}
            {!props.appsLoading && props.apps.length === 0 && (
              <FormField
                label="App id"
                htmlFor="lookup-app-id"
                error={errors.appId?.message}
                hint="Your role cannot list apps, so name the sending app by its id."
              >
                <Input
                  id="lookup-app-id"
                  autoComplete="off"
                  spellCheck={false}
                  className="font-mono"
                  {...register("appId")}
                />
              </FormField>
            )}
          </>
        )}

        {kind === "number" && (
          <FormField
            label="Phone number"
            htmlFor="lookup-msisdn"
            error={errors.msisdn?.message}
            hint="The whole number with its country code. Never kept in the address or copied anywhere on this page."
          >
            <Input
              id="lookup-msisdn"
              type="text"
              inputMode="tel"
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
              placeholder="+237677123456"
              {...register("msisdn")}
            />
          </FormField>
        )}

        {kind !== "id" && (
          <>
            <FormField label="Window" htmlFor="lookup-window" control="group" hint={WINDOW_HINT}>
              <fieldset
                aria-labelledby={groupLabelId("lookup-window")}
                className="m-0 flex min-w-0 flex-wrap items-center gap-2 border-0 p-0"
              >
                {WINDOW_PRESETS.map((preset) => (
                  <FilterChip
                    key={preset.days}
                    selected={props.presetDays === preset.days}
                    onClick={() => props.onPreset(preset.days)}
                  >
                    {preset.label}
                  </FilterChip>
                ))}
              </fieldset>
            </FormField>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <FormField label="From" htmlFor="lookup-from" error={errors.from?.message}>
                <Input
                  id="lookup-from"
                  type="date"
                  {...(props.earliestFrom !== undefined ? { min: props.earliestFrom } : {})}
                  {...register("from")}
                />
              </FormField>
              <FormField label="To" htmlFor="lookup-to" error={errors.to?.message}>
                <Input id="lookup-to" type="date" {...register("to")} />
              </FormField>
            </div>
          </>
        )}

        {props.bannerText !== null && (
          <div role="alert">
            <InlineBanner variant="danger">{props.bannerText}</InlineBanner>
          </div>
        )}

        <div>
          <Button
            type="submit"
            disabled={!props.canSubmit || props.isPending}
            className="h-12 w-full rounded-full px-6 sm:w-auto"
          >
            {props.isPending ? "Looking up…" : "Look up"}
          </Button>
        </div>
      </form>
    </section>
  );
}
