import { EVENT_TYPES, type EventType } from "../webhook-domain";

// Dumb (R6): the event-type multi-select rendered as toggle chips.
export function EventTypeToggles({
  selected,
  onChange,
}: {
  selected: EventType[];
  onChange: (types: EventType[]) => void;
}) {
  function toggle(type: EventType) {
    onChange(selected.includes(type) ? selected.filter((t) => t !== type) : [...selected, type]);
  }
  return (
    <div className="flex flex-wrap gap-1.5">
      {EVENT_TYPES.map((type) => {
        const active = selected.includes(type);
        return (
          <button
            key={type}
            type="button"
            onClick={() => toggle(type)}
            aria-pressed={active}
            className={
              active
                ? "rounded-sm border border-foreground bg-foreground px-2 py-1 font-mono text-background text-caption"
                : "rounded-sm border border-edge px-2 py-1 font-mono text-caption text-muted-foreground hover:border-edge-strong"
            }
          >
            {type}
          </button>
        );
      })}
    </div>
  );
}
