// Dumb — route-local to the message detail screen (R6). Card chrome
// around `ReceiptsTable`.

import { Card, CardBody, CardHeader } from "@vsms/ui";
import type { DeliveryReceiptSummary } from "../message-record";
import { ReceiptsTable } from "./receipts-table";

export interface ReceiptsCardProps {
  receipts: DeliveryReceiptSummary[] | undefined;
  isLoading: boolean;
}

export function ReceiptsCard({ receipts, isLoading }: ReceiptsCardProps) {
  return (
    <Card>
      <CardHeader
        title="Delivery receipts"
        meta="Raw DeliveryReceipt evidence, oldest first — including duplicates and out-of-order arrivals"
      />
      <CardBody>
        <ReceiptsTable receipts={receipts} isLoading={isLoading} />
      </CardBody>
    </Card>
  );
}
