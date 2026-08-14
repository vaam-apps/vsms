// Dumb view: a labelled, pretty-printed JSON block. Formatting itself
// (`prettyJson`) is a pure module the smart layer calls before handing this
// component its already-formatted `value` — this component only renders.

export function JsonBlock({ label, value }: { label: string; value: string | undefined }) {
  if (value === undefined) return null;
  return (
    <div className="flex flex-col gap-1.5">
      <p className="font-medium text-caption text-muted-foreground">{label}</p>
      <pre className="max-h-64 overflow-auto rounded-sm bg-base-100 p-3 font-mono text-[12px] text-foreground">
        {value}
      </pre>
    </div>
  );
}
