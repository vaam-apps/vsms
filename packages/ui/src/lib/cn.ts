import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

/**
 * Merge class names, resolving conflicting Tailwind utility classes
 * (`twMerge`) after conditional composition (`clsx`). daisyUI component
 * classes (`btn`, `card`, …) aren't Tailwind utilities, so `twMerge` passes
 * them through unmodified — it only dedupes the Tailwind utilities layered
 * on top of them.
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
