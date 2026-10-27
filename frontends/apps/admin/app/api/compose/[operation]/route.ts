// The composer's route: `previewMessage` and `sendMessage` through the console's
// own service key, once the browser has proved it is paired
// (`@vsms/gateway/bff`, `composer.ts`, and `compose-proof.ts` for why). Nothing
// else in the console uses the service key.

import { createComposer } from "@vsms/gateway/bff";
import { composerClient } from "@/lib/server/composer-client";
import { gatewayUpstream } from "@/lib/server/gateway-address";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export function POST(request: Request): Promise<Response> {
  return createComposer({
    upstream: gatewayUpstream(),
    client: composerClient,
  })(request);
}
