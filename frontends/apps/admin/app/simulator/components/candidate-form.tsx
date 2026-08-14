// Dumb component (R6): the "Candidate" form card. Markup moved verbatim
// out of `simulator-screen.tsx`. Takes the `react-hook-form`
// control/register it's handed rather than owning the form itself — the
// screen still decides what "Simulate"/"Re-roll" actually do.

import {
  Button,
  Card,
  CardBody,
  CardHeader,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@vsms/ui";
import { type Control, Controller, type UseFormRegister } from "react-hook-form";
import { MESSAGE_CLASSES } from "../message-classes";
import type { SimulateFormValues } from "../simulate-form-values";

export interface CandidateFormProps {
  control: Control<SimulateFormValues>;
  register: UseFormRegister<SimulateFormValues>;
  onRun: () => void;
  onReroll: () => void;
  isFetching: boolean;
  hasRun: boolean;
  canRun: boolean;
}

export function CandidateForm({
  control,
  register,
  onRun,
  onReroll,
  isFetching,
  hasRun,
  canRun,
}: CandidateFormProps) {
  return (
    <Card>
      <CardHeader title="Candidate" meta="Nothing below sends a real message" />
      <CardBody>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sim-msisdn">Recipient (E.164)</Label>
            <Input id="sim-msisdn" placeholder="+237677123456" {...register("msisdn")} />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sim-class">Message class</Label>
            <Controller
              control={control}
              name="messageClass"
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange}>
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
              )}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sim-app-id">App id</Label>
            <Input
              id="sim-app-id"
              placeholder="the App this message would be sent from"
              {...register("appId")}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sim-draw">Draw (0–1, optional — a tie-break replay value)</Label>
            <Input
              id="sim-draw"
              placeholder="leave empty for a fresh random draw"
              {...register("draw")}
            />
          </div>
        </div>

        <div className="mt-4 flex items-center gap-2">
          <Button type="button" onClick={onRun} disabled={isFetching || !canRun}>
            {isFetching ? "Simulating…" : "Simulate"}
          </Button>
          {hasRun && (
            <Button type="button" variant="secondary" onClick={onReroll} disabled={isFetching}>
              Re-roll draw
            </Button>
          )}
        </div>
      </CardBody>
    </Card>
  );
}
