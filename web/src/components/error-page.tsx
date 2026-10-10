// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

import { Logo } from "@/components/logo";
import { SectionLabel } from "@/components/brand";

export function ErrorPage({ code, error }: { code: string; error: string }) {
  return (
    <div className="flex w-full flex-col items-center justify-center gap-5 p-6 text-center">
      <Logo size={120} className="motion-safe:animate-wiggle" />
      <SectionLabel>{code}</SectionLabel>
      <h1 className="text-display font-semibold tracking-display">{error}</h1>
    </div>
  );
}
