import {
  FormField,
  InlineBanner,
  Input,
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
      <FormField label="Name" htmlFor="route-name" error={formState.errors.name?.message}>
        <Input id="route-name" aria-invalid={formState.errors.name != null} {...register("name")} />
      </FormField>

      <div className="grid grid-cols-2 gap-3">
        <FormField
          label="Priority (0–1000, higher wins)"
          htmlFor="route-priority"
          error={formState.errors.priority?.message}
        >
          <Input
            id="route-priority"
            inputMode="numeric"
            aria-invalid={formState.errors.priority != null}
            {...register("priority")}
          />
        </FormField>
        <FormField
          label="Weight (within a priority band)"
          htmlFor="route-weight"
          error={formState.errors.weight?.message}
        >
          <Input
            id="route-weight"
            inputMode="numeric"
            aria-invalid={formState.errors.weight != null}
            {...register("weight")}
          />
        </FormField>
      </div>

      <FormField label="Status" htmlFor="route-enabled">
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
      </FormField>

      <FormField
        label="Provider"
        htmlFor="route-provider"
        error={formState.errors.providerId?.message}
      >
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
      </FormField>

      <p className="text-caption text-muted-foreground">
        Match predicates below — each left as "any" matches every candidate for that field (§6.3:
        `NULL` on a `match*` column is a wildcard, never "matches nothing").
      </p>

      <div className="grid grid-cols-2 gap-3">
        <FormField label="Operator" htmlFor="route-match-operator">
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
        </FormField>
        <FormField label="Message class" htmlFor="route-match-class">
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
        </FormField>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <FormField label="App id" htmlFor="route-match-app-id">
          <Input id="route-match-app-id" placeholder="any" {...register("matchAppId")} />
        </FormField>
        <FormField label="National prefix" htmlFor="route-match-prefix">
          <Input id="route-match-prefix" placeholder="e.g. 677" {...register("matchPrefix")} />
        </FormField>
      </div>

      {saveErrorMessage != null && (
        <InlineBanner variant="danger">Save failed: {saveErrorMessage}</InlineBanner>
      )}
    </form>
  );
}
