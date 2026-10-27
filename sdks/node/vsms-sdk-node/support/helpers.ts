/** What the SDK's suites share: a sealed server around a client, and the assertions on its errors. */
import { strict as assert } from "node:assert";
import { SdkError, VsmsClient } from "../dist/index.js";
import {
  type Reply,
  type SealedHandler,
  type SealedServerOptions,
  startSealedServer,
} from "./server.ts";

export const SEND_RESULT = {
  messageId: "m1",
  state: "queued",
  encoding: "gsm7",
  segments: 1,
  operator: "orange",
  estimatedCostXaf: "15",
};

export const INPUT = { to: "+237677123456", body: "Your code is 4471", senderId: "VYMALO" };

export const ok = (body: unknown, headers?: Record<string, string>): Reply => ({
  status: 200,
  body: JSON.stringify(body),
  ...(headers ? { headers } : {}),
});

/** Runs `body` against a sealed server and always closes it. */
export async function withServer(
  handler: SealedHandler,
  body: (
    server: Awaited<ReturnType<typeof startSealedServer>>,
    client: VsmsClient,
  ) => Promise<void>,
  options?: SealedServerOptions,
): Promise<void> {
  const server = await startSealedServer(handler, options);
  try {
    await body(server, new VsmsClient(server.clientOptions));
  } finally {
    await server.close();
  }
}

export async function rejection(promise: Promise<unknown>): Promise<SdkError> {
  try {
    await promise;
  } catch (err) {
    assert.ok(err instanceof SdkError, `not an SdkError: ${String(err)}`);
    return err;
  }
  assert.fail("expected the call to be rejected");
}
