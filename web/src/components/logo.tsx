// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { cn } from "@/lib/utils";

// The ribbon of the mark, drawn twice with an offset.
const ribbon =
  "m 18.554538,150.48551 c -0.158474,-14.75575 -0.316945,-29.51117 10.95595,-29.78142 11.272894,-0.27025 33.975771,13.94442 45.243999,13.8386 11.268228,-0.10583 11.100442,-14.53243 10.932653,-28.95932";

/**
 * The Rinjani Analytics mark, from the brand kit (`rinjani_logo.svg`). All
 * the products of the family use it next to their own name.
 */
export function Logo({
  className,
  size = 32,
}: {
  className?: string;
  size?: number;
}) {
  return (
    <svg
      viewBox="-1 4 500 500"
      width={size}
      height={size}
      xmlns="http://www.w3.org/2000/svg"
      className={cn("shrink-0 select-none", className)}
      aria-hidden="true"
    >
      <g
        transform="matrix(5.804251,0.05161894,-0.06174394,6.9589776,-114.66292,-662.73691)"
        fill="#009685"
      >
        <path
          opacity="0.687378"
          transform="matrix(1.0442053,7.9087599e-5,1.1342088e-4,1.0548666,2.794921,0.76917208)"
          d={ribbon}
        />
        <path
          opacity="0.687378"
          transform="matrix(1.0442053,7.9087599e-5,1.1342088e-4,1.0548666,16.361549,-8.3967354)"
          d={ribbon}
        />
      </g>
    </svg>
  );
}

/** The mark with the product and company names. */
export function Wordmark({
  className,
  wordmarkClassName,
  size = 32,
}: {
  className?: string;
  wordmarkClassName?: string;
  size?: number;
}) {
  return (
    <span
      className={cn(
        "inline-flex w-full select-none items-center gap-2.5",
        className,
      )}
    >
      <Logo size={size} />
      <span className={cn("min-w-0 leading-tight", wordmarkClassName)}>
        <b className="block truncate text-lg tracking-tight">FlowScope</b>
        <small className="block truncate text-2xs text-muted-foreground">
          Rinjani Analytics
        </small>
      </span>
    </span>
  );
}
