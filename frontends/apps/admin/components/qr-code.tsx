// Dumb component (R6): a QR code drawn as one SVG path, so the page needs no
// script to paint it and no `data:` image or injected HTML to carry it (a
// content-security policy can then allow neither).
//
// Black on white whatever the theme: a scanner reads contrast, not roles, and an
// inverted code in dark mode is one many phones refuse. The white margin (the
// four-module quiet zone the specification asks for) is part of the code.

import qrcode from "qrcode-generator";

const QUIET_ZONE = 4;

export interface QrCodeProps {
  text: string;
  /** What a screen reader says in place of the picture. */
  label: string;
  className?: string;
}

export function QrCode({ text, label, className }: QrCodeProps) {
  const code = qrcode(0, "M");
  code.addData(text);
  code.make();
  const size = code.getModuleCount();
  let path = "";
  for (let row = 0; row < size; row += 1) {
    for (let column = 0; column < size; column += 1) {
      if (code.isDark(row, column)) path += `M${column + QUIET_ZONE} ${row + QUIET_ZONE}h1v1h-1z`;
    }
  }
  const side = size + QUIET_ZONE * 2;
  return (
    <svg
      viewBox={`0 0 ${side} ${side}`}
      role="img"
      aria-label={label}
      shapeRendering="crispEdges"
      className={className}
    >
      <rect width={side} height={side} fill="white" />
      <path d={path} fill="black" />
    </svg>
  );
}
