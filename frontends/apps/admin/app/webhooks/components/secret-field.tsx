import { MaskedValue } from "@vaam-apps/ui";

// Dumb (R6): a masked-by-default secret value. Reveal/copy, and the local
// state either needs, now live inside `MaskedValue` itself — this file no
// longer hand-rolls either. `prefix={6}` is `"whsec_"`'s own length: that
// prefix says what kind of thing this is and isn't itself secret, matching
// this format byte-for-byte (`sms_webhook::generate_secret()`,
// `whsec_<64 hex chars>`, per AGENTS.md's #41/#53 sections).
//
// The deleted `maskSecret` had a test for a value shorter than the tail.
// It is not replaced here, and the reason is worth stating rather than
// leaving as an absence: the library's masking overlaps its head and tail
// slices when `prefix + reveal` exceeds the value's length, so it
// *duplicates* characters -- `"ab"` renders as `"abab"`, `"whsec_"` as
// `"whsec_sec_"` (measured against the installed 0.2.0, filed upstream).
// Unreachable here because this format is fixed at 70 characters and the
// value is server-generated, never user-supplied -- but a shorter secret
// would render wrongly, so do not reuse this component for a free-form
// one without checking that upstream is fixed.
export function SecretField({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col gap-1">
      <p className="text-caption text-subtle-foreground">{label}</p>
      <code className="rounded-sm border border-edge bg-surface-2 px-2 py-1 text-caption text-foreground">
        <MaskedValue value={value} prefix={6} reveal={4} label={label} />
      </code>
    </div>
  );
}
