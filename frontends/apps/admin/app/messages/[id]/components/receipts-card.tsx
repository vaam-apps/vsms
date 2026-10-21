// Dumb — route-local to the message detail screen (R6). Card chrome
// around `ReceiptsTable`.

import { Card, CardBody, CardHeader } from "@vaam-apps/ui";
import type { DeliveryReceiptSummary } from "../message-record";
import { ReceiptsTable } from "./receipts-table";

export interface ReceiptsCardProps {
  receipts: DeliveryReceiptSummary[] | undefined;
  isLoading: boolean;
  errorMessage?: string | undefined;
}

export function ReceiptsCard({ receipts, isLoading, errorMessage }: ReceiptsCardProps) {
  return (
    <Card>
      <CardHeader
        title="Delivery receipts"
        headingLevel={2}
        meta="As the provider sent them, oldest first, duplicates included"
      />
      <CardBody>
        <ReceiptsTable receipts={receipts} isLoading={isLoading} errorMessage={errorMessage} />
      </CardBody>
    </Card>
  );
}
