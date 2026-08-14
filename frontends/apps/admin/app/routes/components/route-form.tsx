import {
  Input,
  Label,
  MESSAGE_CLASSES,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@vsms/ui";
import { Controller, type UseFormReturn } from "react-hook-form";
import {
  ANY_PREDICATE,
  OPERATOR_CODES,
  type ProviderListItem,
  type RouteFormValues,
} from "../route-domain";

// Dumb, route-local (R6): the create/edit form fields. Takes the
// `react-hook-form` instance and provider list as props — it renders markup
// and knows nothing about mutations, routing, or why it's being submitted.
export function RouteForm({
  formId,
  form,
  providers,
  onSubmit,
  saveErrorMessage,
}: {
  formId: string;
  form: UseFormReturn<RouteFormValues>;
  providers: ProviderListItem[] | undefined;
  onSubmit: (values: RouteFormValues) => void;
  saveErrorMessage?: string | undefined;
}) {
  const { register, control, formState, handleSubmit } = form;

  return (
    <form id={formId} onSubmit={handleSubmit(onSubmit)} className="flex flex-col gap-4">
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="route-name">Name</Label>
        <Input id="route-name" aria-invalid={formState.errors.name != null} {...register("name")} />
        {formState.errors.name != null && (
          <p className="text-caption text-state-danger-fg">{formState.errors.name.message}</p>
        )}
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="route-priority">Priority (0–1000, higher wins)</Label>
          <Input
            id="route-priority"
            inputMode="numeric"
            aria-invalid={formState.errors.priority != null}
            {...register("priority")}
          />
          {formState.errors.priority != null && (
            <p className="text-caption text-state-danger-fg">{formState.errors.priority.message}</p>
          )}
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="route-weight">Weight (within a priority band)</Label>
          <Input
            id="route-weight"
            inputMode="numeric"
            aria-invalid={formState.errors.weight != null}
            {...register("weight")}
          />
          {formState.errors.weight != null && (
            <p className="text-caption text-state-danger-fg">{formState.errors.weight.message}</p>
          )}
        </div>
      </div>

      <div className="flex flex-col gap-1.5">
        <Label htmlFor="route-enabled">Status</Label>
        <Controller
          control={control}
          name="enabled"
          render={({ field }) => (
            <Select value={field.value} onValueChange={field.onChange}>
              <SelectTrigger id="route-enabled">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="enabled">Enabled</SelectItem>
                <SelectItem value="disabled">Disabled</SelectItem>
              </SelectContent>
            </Select>
          )}
        />
      </div>

      <div className="flex flex-col gap-1.5">
        <Label htmlFor="route-provider">Provider</Label>
        <Controller
          control={control}
          name="providerId"
          render={({ field }) => (
            <Select value={field.value} onValueChange={field.onChange}>
              <SelectTrigger id="route-provider">
                <SelectValue placeholder="Select a provider" />
              </SelectTrigger>
              <SelectContent>
                {providers?.map((provider) => (
                  <SelectItem key={provider.id} value={provider.id}>
                    {provider.displayName} ({provider.key})
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
        {formState.errors.providerId != null && (
          <p className="text-caption text-state-danger-fg">{formState.errors.providerId.message}</p>
        )}
      </div>

      <p className="text-caption text-muted-foreground">
        Match predicates below — each left as "any" matches every candidate for that field (§6.3:
        `NULL` on a `match*` column is a wildcard, never "matches nothing").
      </p>

      <div className="grid grid-cols-2 gap-3">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="route-match-operator">Operator</Label>
          <Controller
            control={control}
            name="matchOperator"
            render={({ field }) => (
              <Select value={field.value} onValueChange={field.onChange}>
                <SelectTrigger id="route-match-operator">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={ANY_PREDICATE}>Any</SelectItem>
                  {OPERATOR_CODES.map((code) => (
                    <SelectItem key={code} value={code}>
                      {code}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="route-match-class">Message class</Label>
          <Controller
            control={control}
            name="matchClass"
            render={({ field }) => (
              <Select value={field.value} onValueChange={field.onChange}>
                <SelectTrigger id="route-match-class">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={ANY_PREDICATE}>Any</SelectItem>
                  {MESSAGE_CLASSES.map((cls) => (
                    <SelectItem key={cls} value={cls}>
                      {cls}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          />
        </div>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="route-match-app-id">App id</Label>
          <Input id="route-match-app-id" placeholder="any" {...register("matchAppId")} />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="route-match-prefix">National prefix</Label>
          <Input id="route-match-prefix" placeholder="e.g. 677" {...register("matchPrefix")} />
        </div>
      </div>

      {saveErrorMessage != null && (
        <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
          Save failed: {saveErrorMessage}
        </div>
      )}
    </form>
  );
}
