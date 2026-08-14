// Dumb component (R6): the Providers screen's own title block. Markup
// moved verbatim out of `providers-screen.tsx`.

export function ProvidersHeader() {
  return (
    <header className="flex flex-col gap-1 border-edge border-b pb-6">
      <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
        vsms admin console
      </p>
      <h1 className="font-medium text-foreground text-title">Providers</h1>
      <p className="max-w-xl text-body text-muted-foreground">
        Every configured SMS provider — capacity, cost, and current state.
      </p>
    </header>
  );
}
