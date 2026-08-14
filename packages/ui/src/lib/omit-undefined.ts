/**
 * Strips every key whose value is explicitly `undefined`, and narrows the
 * return type to match: `exactOptionalPropertyTypes` (this workspace's own
 * `tsconfig`) makes `{ disabled?: boolean }` reject a spread whose *type*
 * is `{ disabled?: boolean | undefined }` even when the value at runtime is
 * always defined for the keys that matter — every `React.*HTMLAttributes`
 * type declares its optional fields as `T | undefined` explicitly, but
 * Headless UI's own prop types (`ListboxProps`, `MenuItemProps`, `LabelProps`,
 * …) declare theirs as plain `T`. Spreading `...props` from a native-element
 * prop type onto a Headless UI component hits this every time (found
 * porting `select.tsx`/`dropdown-menu.tsx`/`label.tsx` off Radix, D3) —
 * this is the one general fix rather than a one-off cast at each call site.
 */
export function omitUndefined<T extends Record<string, unknown>>(
  props: T,
): { [K in keyof T]: Exclude<T[K], undefined> } {
  const result = {} as { [K in keyof T]: Exclude<T[K], undefined> };
  for (const key of Object.keys(props) as (keyof T)[]) {
    const value = props[key];
    if (value !== undefined) {
      result[key] = value as Exclude<T[keyof T], undefined>;
    }
  }
  return result;
}
