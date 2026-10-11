// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

import type { ReactNode } from "react";
import { Logo } from "@/components/logo";
import { SectionLabel } from "@/components/brand";

/** A page for an error, with what happened and the ways out as children. */
export function ErrorPage({
  code,
  error,
  description,
  children,
}: {
  code: string;
  error: string;
  description?: string;
  children?: ReactNode;
}) {
  return (
    <div className="flex w-full flex-col items-center justify-center gap-5 p-6 text-center">
      <Logo size={96} className="motion-safe:animate-wiggle" />
      <SectionLabel>{code}</SectionLabel>
      <h1 className="text-title font-semibold leading-tight tracking-display sm:text-display">
        {error}
      </h1>
      {description && (
        <p className="max-w-[46ch] text-muted-foreground">{description}</p>
      )}
      {children && (
        <div className="mt-1 flex flex-wrap justify-center gap-3">
          {children}
        </div>
      )}
    </div>
  );
}
