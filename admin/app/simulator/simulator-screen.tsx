"use client";

// The Route Simulator (#54): "given this recipient, class and app, which
// route wins and why" — without sending anything.
//
// # This screen renders the real engine's answer; it never recomputes one
//
// Every field below comes straight off `routeSimulator.simulate`'s
// response, which is itself `POST /$procs/simulateRoute` — a real call
// into `crates/sms-api/src/procedures.rs::Procedures::simulate`, which
// reads real `Route`/`Provider` rows and calls the real
// `sms_routing::select_route` (`crates/sms-routing`, #62's pure engine).
// Nothing in this file re-implements priority bands, predicate matching,
// or the weighted draw — see `crates/sms-api/src/route_simulator.rs`'s own
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

import { trpc } from "@vsms/hooks";
import {
  Button,
  Card,
  CardBody,
  CardHeader,
  IdDisplay,
  Input,
  Label,
  MsisdnDisplay,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  ThemeToggle,
} from "@vsms/ui";
import { useState } from "react";

const MESSAGE_CLASSES = ["otp", "transactional", "notification", "marketing"] as const;
type MessageClass = (typeof MESSAGE_CLASSES)[number];

type OutcomeKind =
  | "excluded"
  | "disabled"
  | "predicate_failed"
  | "provider_unavailable"
  | "eligible";

const OUTCOME_LABELS: Record<OutcomeKind, string> = {
  excluded: "Excluded",
  disabled: "Disabled",
  predicate_failed: "Predicate failed",
  provider_unavailable: "Provider unavailable",
  eligible: "Eligible",
};

const OUTCOME_CLASSES: Record<OutcomeKind, string> = {
  excluded: "border-edge-strong bg-surface-2 text-muted-foreground",
  disabled: "border-edge-strong bg-surface-2 text-muted-foreground",
  predicate_failed: "border-state-uncertain-border bg-state-uncertain-bg text-state-uncertain-fg",
  provider_unavailable: "border-state-danger-border bg-state-danger-bg text-state-danger-fg",
  eligible: "border-state-success-border bg-state-success-bg text-state-success-fg",
};

function OutcomePill({ outcome }: { outcome: OutcomeKind }) {
  return (
    <span className={`rounded-sm border px-1.5 py-0.5 text-caption ${OUTCOME_CLASSES[outcome]}`}>
      {OUTCOME_LABELS[outcome]}
    </span>
  );
}

const PREDICATE_LABELS: Record<string, string> = {
  operator: "Operator",
  class: "Message class",
  app_id: "App",
  prefix: "Prefix",
};

export function SimulatorScreen() {
  const [msisdn, setMsisdn] = useState("+237677123456");
  const [messageClass, setMessageClass] = useState<MessageClass>("otp");
  const [appId, setAppId] = useState("");
  const [draw, setDraw] = useState("");
  const [submitted, setSubmitted] = useState(false);

  const query = trpc.routeSimulator.simulate.useQuery(
    {
      msisdn,
      class: messageClass,
      appId,
      ...(draw !== "" ? { draw: Number(draw) } : {}),
    },
    { enabled: false },
  );

  function run() {
    setSubmitted(true);
    void query.refetch();
  }

  function reroll() {
    setDraw("");
    setSubmitted(true);
    void query.refetch();
  }

  const result = query.data;
  const winnerRouteId = result?.winner?.routeId;

  return (
    <main className="mx-auto flex max-w-[1100px] flex-col gap-6 px-6 py-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            vsms admin console
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Route simulator</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            Given a recipient, message class, and app, which route wins and why — without sending
            anything. Renders the real routing engine's own decision.
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          <a
            href="/dashboard"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Dashboard
          </a>
          <a
            href="/providers"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Providers
          </a>
          <a
            href="/routes"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Routes
          </a>
          <a
            href="/"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Composer
          </a>
          <a
            href="/sender-ids"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Sender IDs
          </a>
          <a
            href="/webhooks"
            className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
          >
            Webhooks
          </a>
          <ThemeToggle />
        </div>
      </header>

      <Card>
        <CardHeader title="Candidate" meta="Nothing below sends a real message" />
        <CardBody>
          <div className="grid grid-cols-2 gap-4">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="sim-msisdn">Recipient (E.164)</Label>
              <Input
                id="sim-msisdn"
                value={msisdn}
                onChange={(e) => setMsisdn(e.target.value)}
                placeholder="+237677123456"
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="sim-class">Message class</Label>
              <Select
                value={messageClass}
                onValueChange={(value) => setMessageClass(value as MessageClass)}
              >
                <SelectTrigger id="sim-class">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {MESSAGE_CLASSES.map((cls) => (
                    <SelectItem key={cls} value={cls}>
                      {cls}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="sim-app-id">App id</Label>
              <Input
                id="sim-app-id"
                value={appId}
                onChange={(e) => setAppId(e.target.value)}
                placeholder="the App this message would be sent from"
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="sim-draw">Draw (0–1, optional — a tie-break replay value)</Label>
              <Input
                id="sim-draw"
                value={draw}
                onChange={(e) => setDraw(e.target.value)}
                placeholder="leave empty for a fresh random draw"
              />
            </div>
          </div>

          <div className="mt-4 flex items-center gap-2">
            <Button type="button" onClick={run} disabled={query.isFetching || appId === ""}>
              {query.isFetching ? "Simulating…" : "Simulate"}
            </Button>
            {submitted && (
              <Button
                type="button"
                variant="secondary"
                onClick={reroll}
                disabled={query.isFetching}
              >
                Re-roll draw
              </Button>
            )}
          </div>
        </CardBody>
      </Card>

      {query.isError && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Simulation failed: {query.error.message}
        </div>
      )}

      {submitted && query.isFetching && <Skeleton className="h-40 w-full" />}

      {result?.noRoutesConfigured === true && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          No <span className="font-mono">Route</span> rows exist in this system at all — every
          message would be rejected, loudly (§62's own "dispatch refuses, not silently falls back").
          This is distinct from "routes exist, none matched" below — there is nothing at all to
          evaluate here.
        </div>
      )}

      {result !== undefined && !result.noRoutesConfigured && (
        <>
          <Card>
            <CardHeader title="Result" meta={`Classified operator: ${result.operator}`} />
            <CardBody className="flex flex-col gap-3">
              <MsisdnDisplay value={msisdn} operator={result.operator} />

              {result.winner !== undefined ? (
                <div className="rounded-sm border border-state-success-border bg-state-success-bg px-3 py-3 text-state-success-fg">
                  <p className="font-medium text-body">
                    Winner: route <IdDisplay value={result.winner.routeId} />
                  </p>
                  <p className="mt-1 text-caption">
                    Provider <IdDisplay value={result.winner.providerId} />
                    {result.winner.failoverRouteId !== undefined && (
                      <>
                        {" "}
                        · failover route <IdDisplay value={result.winner.failoverRouteId} />
                      </>
                    )}
                  </p>
                </div>
              ) : (
                <div className="rounded-sm border border-state-uncertain-border bg-state-uncertain-bg px-3 py-3 text-caption text-state-uncertain-fg">
                  No eligible route for this candidate — every evaluated route below was excluded,
                  disabled, failed a predicate, or had no available provider.
                </div>
              )}

              {result.tieBreak !== undefined && (
                <div className="rounded-sm border border-edge bg-surface-2 p-3">
                  <p className="text-caption text-muted-foreground">
                    Tie-break within priority {result.tieBreak.priority} — draw{" "}
                    <span className="font-mono text-foreground">
                      {result.tieBreak.draw.toFixed(4)}
                    </span>
                  </p>
                  <div className="mt-2 flex h-6 w-full overflow-hidden rounded-sm border border-edge">
                    {result.tieBreak.ranges.map((range) => {
                      const isWinner = range.routeId === result.tieBreak?.winnerRouteId;
                      const widthPct = (range.high - range.low) * 100;
                      return (
                        <div
                          key={range.routeId}
                          className={
                            isWinner
                              ? "flex items-center justify-center border-edge border-r bg-state-success-bg text-state-success-fg last:border-r-0"
                              : "flex items-center justify-center border-edge border-r bg-surface-3 text-muted-foreground last:border-r-0"
                          }
                          style={{ width: `${widthPct}%` }}
                          title={`route ${range.routeId} — weight ${range.weight} — [${range.low.toFixed(3)}, ${range.high.toFixed(3)})`}
                        >
                          <span className="truncate px-1 font-mono text-[10px]">
                            w={range.weight}
                          </span>
                        </div>
                      );
                    })}
                  </div>
                </div>
              )}
            </CardBody>
          </Card>

          <Table>
            <TableHeader>
              <TableRow>
                <TableHead align="end">Priority</TableHead>
                <TableHead align="end">Weight</TableHead>
                <TableHead>Route</TableHead>
                <TableHead>Outcome</TableHead>
                <TableHead>Detail</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {result.evaluations.map((evaluation) => (
                <TableRow key={evaluation.routeId} selected={evaluation.routeId === winnerRouteId}>
                  <TableCell align="end" mono>
                    {evaluation.priority}
                  </TableCell>
                  <TableCell align="end" mono>
                    {evaluation.weight}
                  </TableCell>
                  <TableCell>
                    {evaluation.routeName}
                    {evaluation.routeId === winnerRouteId && (
                      <span className="ml-2 rounded-sm border border-state-success-border bg-state-success-bg px-1.5 py-0.5 text-caption text-state-success-fg">
                        winner
                      </span>
                    )}
                  </TableCell>
                  <TableCell>
                    <OutcomePill outcome={evaluation.outcome} />
                  </TableCell>
                  <TableCell className="text-caption text-muted-foreground">
                    {evaluation.outcome === "predicate_failed" &&
                      evaluation.predicateKind !== undefined && (
                        <>
                          {PREDICATE_LABELS[evaluation.predicateKind]}: expected{" "}
                          <span className="font-mono text-foreground">
                            {evaluation.predicateExpected}
                          </span>
                          , candidate is{" "}
                          <span className="font-mono text-foreground">
                            {evaluation.predicateActual}
                          </span>
                        </>
                      )}
                    {evaluation.outcome === "provider_unavailable" && evaluation.unavailableReason}
                    {evaluation.outcome === "eligible" &&
                      (evaluation.winningBand
                        ? "in the winning priority band"
                        : "outranked by a higher-priority band")}
                    {(evaluation.outcome === "excluded" || evaluation.outcome === "disabled") &&
                      "—"}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </>
      )}
    </main>
  );
}
