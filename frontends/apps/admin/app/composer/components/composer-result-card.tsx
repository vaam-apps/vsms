// Dumb — route-local to the composer (R6). The "message accepted" summary
// shown after a successful `compose.send`.

import { Card, CardBody, CardHeader, StatusPill } from "@vsms/ui";
import type { ComposeSendResult } from "../composer-types";

export interface ComposerResultCardProps {
  result: ComposeSendResult;
}

export function ComposerResultCard({ result }: ComposerResultCardProps) {
  return (
    <Card>
      <CardHeader title="Message accepted" meta={result.messageId} />
      <CardBody className="flex flex-wrap items-center gap-3">
        <StatusPill state={result.state} showLiteral />
        <span className="font-mono text-caption text-muted-foreground">
          {result.encoding.toUpperCase()} · {result.segments} seg
        </span>
        <span className="font-mono text-caption text-muted-foreground">
          {result.operator === "unknown" ? "operator unknown" : result.operator.toUpperCase()}
        </span>
        <span className="font-mono text-caption text-muted-foreground">
          ~{result.estimatedCostXaf} FCFA
        </span>
      </CardBody>
    </Card>
  );
}
