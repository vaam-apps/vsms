"use client";

// The Route Simulator (#54): "given this recipient, class and app, which
// route wins and why" — without sending anything.
//
// # This screen renders the real engine's answer; it never recomputes one
//
// Every field below comes straight off `routeSimulator.simulate`'s
// response, which is itself `POST /$procs/simulateRoute` — a real call
// into `backends/crates/sms-api/src/procedures.rs::Procedures::simulate`, which
// reads real `Route`/`Provider` rows and calls the real
// `sms_routing::select_route` (`backends/crates/sms-routing`, #62's pure engine).
// Nothing in this file re-implements priority bands, predicate matching,
// or the weighted draw — see `backends/crates/sms-api/src/route_simulator.rs`'s own
// module doc for the test that proves the wire shape can't silently drift
// from what the engine actually decided. If this screen and a real
// dispatch ever disagreed, `route_simulator.rs`'s guard is what would have
// to be broken first — a fact worth stating because a client-side
// reimplementation, however careful, could drift from the engine the
// moment either changed, and would then confidently show the wrong answer.
//
// # The zero-routes state gets its own, unmissable banner
//
// `noRoutesConfigured` is `true` only when the whole system has zero
// `Route` rows — the #62-documented "dispatch refuses every message,
// loudly" state — distinct from "routes exist, none matched this
// candidate" (`evaluations` non-empty, `winner` absent). Collapsing the two
// into one empty-looking table is exactly the failure mode this ticket's
// own trap list warns against.
//
// # The injected draw, made visible
//
// `sms_routing::select_route`'s own doc: the random draw is injected, not
// generated internally, "precisely so a simulator can replay a decision
// deterministically." Leaving the draw field empty asks the server for a
// fresh, realistic sample (`rand::random()`, the same call production
// dispatch makes); "Re-roll" does the same again. Pinning a draw and
// resubmitting reproduces the identical winner — the tie-break panel below
// renders the exact `[low, high)` ranges and the draw's landing position
// the engine itself reported, not a client-side re-derivation of them.
//
// # R6
//
// This file holds data fetching, form wiring and derived values only —
// every class and every piece of markup lives in `./components/*`.
// `hasRun` replaces a former `submitted` useState: `query.isFetching ||
// query.isFetched` is true in exactly the same cases the old boolean was
// set, without a second source of truth to keep in sync with the query
// itself. `msisdn`/`messageClass`/`appId`/`draw` moved from four separate
// `useState` calls onto one `react-hook-form` instance (R6: "form state →
// react-hook-form + zod") — no zod resolver, since the original screen
// never validated these fields beyond disabling "Simulate" while `appId`
// was empty, which stays a plain derived value below; adding validation
// errors that didn't exist before would be a behaviour change this pass
// isn't making.

import { trpc } from "@vsms/hooks";
import { useForm, useWatch } from "react-hook-form";
import { CandidateForm } from "./components/candidate-form";
import { EvaluationsTable } from "./components/evaluations-table";
import { ResultSummaryCard } from "./components/result-summary-card";
import { SimulatorView } from "./components/simulator-view";
import { SIMULATE_FORM_DEFAULTS, type SimulateFormValues } from "./simulate-form-values";

export function SimulatorScreen() {
  const form = useForm<SimulateFormValues>({ defaultValues: SIMULATE_FORM_DEFAULTS });
  const values = useWatch({ control: form.control, defaultValue: SIMULATE_FORM_DEFAULTS });

  const msisdn = values.msisdn ?? SIMULATE_FORM_DEFAULTS.msisdn;
  const messageClass = values.messageClass ?? SIMULATE_FORM_DEFAULTS.messageClass;
  const appId = values.appId ?? SIMULATE_FORM_DEFAULTS.appId;
  const draw = values.draw ?? SIMULATE_FORM_DEFAULTS.draw;

  const query = trpc.routeSimulator.simulate.useQuery(
    {
      msisdn,
      class: messageClass,
      appId,
      ...(draw !== "" ? { draw: Number(draw) } : {}),
    },
    { enabled: false },
  );

  // Replaces a former `submitted` useState: true in exactly the cases the
  // old boolean was set (immediately on the first "Simulate"/"Re-roll"
  // click, staying true afterward), derived from the query itself rather
  // than a second, hand-kept flag.
  const hasRun = query.isFetching || query.isFetched;

  function run() {
    void query.refetch();
  }

  function reroll() {
    form.setValue("draw", "");
    void query.refetch();
  }

  const result = query.data;
  const winnerRouteId = result?.winner?.routeId;

  return (
    <SimulatorView
      candidateForm={
        <CandidateForm
          control={form.control}
          register={form.register}
          onRun={run}
          onReroll={reroll}
          isFetching={query.isFetching}
          hasRun={hasRun}
          canRun={appId !== ""}
        />
      }
      errorMessage={query.isError ? `Simulation failed: ${query.error.message}` : null}
      isFetchingFirstResult={hasRun && query.isFetching}
      noRoutesConfigured={result?.noRoutesConfigured === true}
      result={
        result !== undefined &&
        !result.noRoutesConfigured && (
          <>
            <ResultSummaryCard
              msisdn={msisdn}
              operator={result.operator}
              winner={result.winner}
              tieBreak={result.tieBreak}
            />
            <EvaluationsTable rows={result.evaluations} winnerRouteId={winnerRouteId} />
          </>
        )
      }
    />
  );
}
