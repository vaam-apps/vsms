// Dumb — route-local to the composer (R6). Static title block.

export function ComposerHeader() {
  return (
    <header className="flex flex-col gap-1 border-edge border-b pb-6">
      <h1 className="font-medium text-foreground text-title">Composer</h1>
      <p className="max-w-md text-body text-muted-foreground">
        See exactly what a message will cost before you send it — GSM-7 vs UCS-2, segment count, and
        every character that would force the more expensive encoding.
      </p>
    </header>
  );
}
