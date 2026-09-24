import {
  Button,
  Dialog,
  DialogActions,
  DialogClose,
  DialogDescription,
  DialogFullScreen,
  DialogHeader,
  DialogTitle,
} from "@vaam-apps/ui";
import type { UseFormReturn } from "react-hook-form";
import type { CreateEndpointFormValues, EventType } from "../webhook-domain";
import { CreateEndpointFields } from "./create-endpoint-fields";

// Dumb (R6): the "New webhook endpoint" dialog, start to finish. Not
// affected by the nested-Dialog-in-drawer bug (see webhooks-screen.tsx's
// own module doc) — it opens from the toolbar while no drawer is open, so
// it stays a real `Dialog`: `DialogFullScreen`, because it is a form of
// five inputs (AGENTS.md's rule for which presentation a dialog takes) —
// full-screen below 640px, centred from 640px up.
export function CreateEndpointDialog({
  open,
  onOpenChange,
  form,
  eventTypes,
  onEventTypesChange,
  onSubmit,
  pending,
  errorMessage,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  form: UseFormReturn<CreateEndpointFormValues>;
  eventTypes: EventType[];
  onEventTypesChange: (types: EventType[]) => void;
  onSubmit: (values: CreateEndpointFormValues) => void;
  pending: boolean;
  errorMessage?: string | undefined;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogFullScreen>
        <DialogHeader>
          <DialogTitle>New webhook endpoint</DialogTitle>
          <DialogDescription>
            A signing secret is generated automatically and shown once creation completes.
          </DialogDescription>
        </DialogHeader>
        <CreateEndpointFields
          formId="create-endpoint-form"
          form={form}
          eventTypes={eventTypes}
          onEventTypesChange={onEventTypesChange}
          onSubmit={onSubmit}
          createErrorMessage={errorMessage}
        />
        <DialogActions>
          <DialogClose as={Button} variant="ghost">
            Cancel
          </DialogClose>
          <Button
            type="submit"
            form="create-endpoint-form"
            disabled={pending || eventTypes.length === 0}
          >
            {pending ? "Creating…" : "Create"}
          </Button>
        </DialogActions>
      </DialogFullScreen>
    </Dialog>
  );
}
