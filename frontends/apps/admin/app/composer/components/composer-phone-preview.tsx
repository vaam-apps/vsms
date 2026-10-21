// Dumb component (R6, route-local): how the message will look on a handset,
// beside the form. The sender id as the thread title, the body as the
// incoming bubble, and the encoding verdict underneath, so the cost and
// the content are read together instead of in two places down the form.
// Purely presentational: every value is handed down by the screen.

export interface ComposerPhonePreviewProps {
  /** The sender the recipient will see; empty means the app's default. */
  senderId: string;
  /** The recipient, normalised when the gateway has answered. */
  to: string;
  body: string;
  /** `null` until the gateway has previewed a non-empty body. */
  encoding: "gsm7" | "ucs2" | null;
  segments: number | null;
}

export function ComposerPhonePreview({
  senderId,
  to,
  body,
  encoding,
  segments,
}: ComposerPhonePreviewProps) {
  return (
    <figure className="flex flex-col items-center gap-3" aria-label="Message preview">
      <div className="w-full max-w-[320px] rounded-[40px] border border-edge-subtle bg-surface-container-low p-3 shadow-sm">
        <div className="flex min-h-[420px] flex-col rounded-[30px] bg-base-100">
          <div className="flex flex-col items-center gap-1 border-edge-subtle border-b px-4 pt-5 pb-3">
            <span
              aria-hidden="true"
              className="flex size-10 items-center justify-center rounded-full bg-primary-container font-medium text-on-primary-container text-title-medium"
            >
              {(senderId.trim()[0] ?? "S").toUpperCase()}
            </span>
            <span className="text-title-small text-foreground">
              {senderId.trim() === "" ? "Default sender" : senderId}
            </span>
            <span className="font-mono text-label-small text-muted-foreground">
              to {to.trim() === "" ? "—" : to}
            </span>
          </div>
          <div className="flex flex-1 flex-col justify-end p-4">
            {body.trim() === "" ? (
              <p className="text-center text-body-small text-muted-foreground">
                Start typing to see the message here.
              </p>
            ) : (
              <p className="max-w-[85%] self-start whitespace-pre-wrap break-words rounded-[20px] rounded-bl-[6px] bg-surface-3 px-4 py-2.5 text-body-medium text-foreground">
                {body}
              </p>
            )}
          </div>
        </div>
      </div>
      <figcaption className="text-body-small text-muted-foreground">
        {encoding === null || segments === null
          ? "Encoding and cost appear as you type."
          : `${encoding === "gsm7" ? "GSM-7" : "UCS-2"} · ${segments} ${segments === 1 ? "segment" : "segments"}`}
      </figcaption>
    </figure>
  );
}
