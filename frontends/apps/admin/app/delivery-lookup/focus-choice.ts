// Pure module (R6): where focus goes when an error belongs to a group of
// radios. A field that is `register`ed takes focus from react-hook-form's
// `setError(..., { shouldFocus: true })`; the app picker is a `RadioGroup`
// driven by `setValue`, which is not registered, so nothing moves focus to it
// and a keyboard or screen-reader user is left where the submit was.

/** The one thing this reads of an element. */
export interface Focusable {
  focus: () => void;
}

/** A container that can find a radio inside it. */
export interface RadioRoot {
  querySelector: (selector: string) => Focusable | null;
}

/** Focuses the radio a keyboard would land on (the checked one, which holds
 * the roving tab stop, or else the first). Returns whether anything took
 * focus; a missing container (the group is not on the page) is `false`. */
export function focusChoice(root: RadioRoot | null): boolean {
  if (root === null) return false;
  const target =
    root.querySelector('[role="radio"][tabindex="0"]') ?? root.querySelector('[role="radio"]');
  if (target === null) return false;
  target.focus();
  return true;
}
