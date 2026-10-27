// What the browser needs from the console's server before it can sign: the
// gateway's two PUBLIC sealing keys, pinned so that the browser believes only an
// answer the gateway signed. The server reads them from its environment
// (`SMS_ENVELOPE_SERVER_KEY_CURRENT`/`_NEXT`, `sms-gateway server-keys` prints
// them) at request time, never at build time, so one image promotes through
// environments; the root layout hands them down as props.

/** The gateway's response-signing keys, 32 bytes each as unpadded base64url. */
export interface ConsoleConfig {
  serverKeys: { current: string; next: string };
  /** `MESSAGE_STREAM_POLL_MS`, for the router that now runs in the page. */
  messageStreamPollMs: number;
}
