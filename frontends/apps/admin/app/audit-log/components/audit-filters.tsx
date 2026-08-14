// Dumb view: the audit log's filter row. Receives the current filter
// values and change callbacks; owns none of the `nuqs` URL-state wiring
// itself.

import {
  Button,
  FormField,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@vsms/ui";
import { type AuditOperation, OPERATIONS } from "../types";

export function AuditFilters({
  model,
  operation,
  actorId,
  since,
  until,
  hasFilters,
  onModelChange,
  onOperationChange,
  onActorIdChange,
  onSinceChange,
  onUntilChange,
  onClear,
}: {
  model: string;
  operation: AuditOperation | null;
  actorId: string;
  since: string;
  until: string;
  hasFilters: boolean;
  onModelChange: (value: string) => void;
  onOperationChange: (value: AuditOperation | null) => void;
  onActorIdChange: (value: string) => void;
  onSinceChange: (value: string) => void;
  onUntilChange: (value: string) => void;
  onClear: () => void;
}) {
  return (
    <div className="flex flex-wrap items-end gap-3">
      <FormField label="Model" htmlFor="audit-filter-model">
        <Input
          id="audit-filter-model"
          placeholder="App, User, Provider…"
          value={model}
          onChange={(e) => onModelChange(e.target.value)}
          className="w-44"
        />
      </FormField>
      <FormField label="Operation" htmlFor="audit-filter-operation">
        <Select
          value={operation ?? "any"}
          onValueChange={(value) =>
            onOperationChange(value === "any" ? null : (value as AuditOperation))
          }
        >
          <SelectTrigger id="audit-filter-operation" className="w-36">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="any">Any</SelectItem>
            {OPERATIONS.map((op) => (
              <SelectItem key={op} value={op}>
                {op}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </FormField>
      <FormField label="Actor id" htmlFor="audit-filter-actor">
        <Input
          id="audit-filter-actor"
          value={actorId}
          onChange={(e) => onActorIdChange(e.target.value)}
          className="w-44"
        />
      </FormField>
      <FormField label="Since" htmlFor="audit-filter-since">
        <Input
          id="audit-filter-since"
          type="date"
          value={since}
          onChange={(e) => onSinceChange(e.target.value)}
        />
      </FormField>
      <FormField label="Until" htmlFor="audit-filter-until">
        <Input
          id="audit-filter-until"
          type="date"
          value={until}
          onChange={(e) => onUntilChange(e.target.value)}
        />
      </FormField>
      {hasFilters && (
        <Button type="button" variant="ghost" size="sm" onClick={onClear}>
          Clear filters
        </Button>
      )}
    </div>
  );
}
