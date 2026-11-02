// Pure module (R6): what keeps one gesture from becoming two budgeted lookups.
//
// A lookup by number or client reference spends one of 60 hourly lookups. The
// submit button is disabled while a lookup is pending, but only after React has
// rendered, and TanStack Query renders a macrotask after `mutate`. Two submits
// that land before that render (a held Enter, a scripted double submit) both
// reach the gateway. So the screen keeps a flag in a ref, which changes at once,
// and goes through `runOnce`; and a held Enter never reaches the form at all.

/** A flag whose change is seen at once. The screen holds it in a `useRef`: a
 * `useState` would change only at the next render, which is the gap. */
export interface InFlight {
  current: boolean;
}

/** Runs `run` unless a lookup is already in flight, and marks one in flight
 * before it does. Returns whether it ran. The flag is cleared by whoever
 * settles the lookup (the mutation's `onSettled`), except when `run` throws
 * before anything was sent: then it is cleared here, or the form would be dead. */
export function runOnce(flag: InFlight, run: () => void): boolean {
  if (flag.current) return false;
  flag.current = true;
  try {
    run();
  } catch (error) {
    flag.current = false;
    throw error;
  }
  return true;
}

/** The part of a keyboard event this reads. */
export interface KeyPress {
  key: string;
  repeat: boolean;
  preventDefault: () => void;
}

/** Swallows an Enter the keyboard is repeating. Implicit submission fires on
 * every repeat of a held Enter, so holding it in the number field would ask
 * once per repeat. The first press (`repeat` false) goes through. */
export function ignoreHeldEnter(event: KeyPress): void {
  if (event.key === "Enter" && event.repeat) event.preventDefault();
}
