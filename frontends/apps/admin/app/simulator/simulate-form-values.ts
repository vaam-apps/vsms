// Pure type for the simulator's candidate form (#54), backing the
// `react-hook-form` instance the screen owns. No zod resolver: the
// original screen never validated these fields beyond disabling "Simulate"
// while `appId` was empty (still true here, computed in the screen as a
// derived value) — adding validation errors that didn't exist before would
// be a behaviour change this R6 pass isn't making.

import type { MessageClass } from "./message-classes";

export interface SimulateFormValues {
  msisdn: string;
  messageClass: MessageClass;
  appId: string;
  draw: string;
}

export const SIMULATE_FORM_DEFAULTS: SimulateFormValues = {
  msisdn: "+237677123456",
  messageClass: "otp",
  appId: "",
  draw: "",
};
