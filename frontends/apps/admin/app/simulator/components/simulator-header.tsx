// Dumb component (R6): the Route simulator's own title block. Markup moved
// verbatim out of `simulator-screen.tsx`.

export function SimulatorHeader() {
  return (
    <header className="flex flex-col gap-1 border-edge border-b pb-6">
      <h1 className="font-medium text-foreground text-title">Route simulator</h1>
      <p className="max-w-xl text-body text-muted-foreground">
        Given a recipient, message class, and app, which route wins and why — without sending
        anything. Renders the real routing engine's own decision.
      </p>
    </header>
  );
}
