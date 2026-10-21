// Dumb component (R6), app-wide: a Next `Link` drawn as a button, for an
// action that is navigation ("Compose", "Back to messages"). A `<button>`
// that calls `router.push` would lose middle-click and the URL on hover;
// `buttonVariants` is exported by `@vaam-apps/ui` for exactly this.

import { type ButtonSize, type ButtonVariant, buttonVariants } from "@vaam-apps/ui";
import Link from "next/link";
import type { ReactNode } from "react";

export interface LinkButtonProps {
  href: string;
  children: ReactNode;
  variant?: ButtonVariant | undefined;
  size?: ButtonSize | undefined;
  "aria-label"?: string | undefined;
}

export function LinkButton({
  href,
  children,
  variant = "secondary",
  size,
  ...aria
}: LinkButtonProps) {
  return (
    <Link href={href} className={buttonVariants({ variant, size })} {...aria}>
      {children}
    </Link>
  );
}
