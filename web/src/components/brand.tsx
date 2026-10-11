// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Building blocks of the Rinjani Analytics brand kit.

import type { ReactNode } from "react";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { cn } from "@/lib/utils";

/** A small uppercase label in a pill, above a title. */
export function SectionLabel({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  return (
    <span
      data-slot="section-label"
      className={cn(
        "inline-flex h-6 w-fit shrink-0 items-center rounded-full bg-accent px-3 text-2xs font-semibold uppercase leading-none tracking-label text-accent-foreground",
        className,
      )}
    >
      {children}
    </span>
  );
}

/** The header of a page: label, title, description and actions. */
export function PageHeader({
  eyebrow,
  title,
  description,
  actions,
  dense,
}: {
  eyebrow: ReactNode;
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  /** Keep the description for a screen with room for it. */
  dense?: boolean;
}) {
  return (
    <div className="flex flex-wrap items-end justify-between gap-x-6 gap-y-3">
      <div className="flex min-w-0 flex-col gap-2.5">
        <SectionLabel>{eyebrow}</SectionLabel>
        <h1 className="text-title font-semibold leading-tight tracking-display sm:text-display">
          {title}
        </h1>
        {description && (
          <p
            className={cn(
              "max-w-measure text-sm text-muted-foreground",
              dense && "hidden sm:block",
            )}
          >
            {description}
          </p>
        )}
      </div>
      {actions && (
        <div className="flex flex-wrap items-center gap-2">{actions}</div>
      )}
    </div>
  );
}

/** A card with an optional label, title, description and action. */
export function Panel({
  eyebrow,
  title,
  description,
  action,
  children,
  size,
  className,
  contentClassName,
}: {
  eyebrow?: ReactNode;
  title?: ReactNode;
  description?: ReactNode;
  action?: ReactNode;
  children: ReactNode;
  size?: "default" | "sm";
  className?: string;
  contentClassName?: string;
}) {
  return (
    <Card size={size} className={className}>
      {(eyebrow || title || action) && (
        <CardHeader className="gap-2">
          {eyebrow && <SectionLabel>{eyebrow}</SectionLabel>}
          {title && (
            <CardTitle>
              <h2>{title}</h2>
            </CardTitle>
          )}
          {description && <CardDescription>{description}</CardDescription>}
          {action && (
            <CardAction className="flex flex-wrap items-center justify-end gap-2">
              {action}
            </CardAction>
          )}
        </CardHeader>
      )}
      <CardContent className={cn("flex min-w-0 flex-col gap-4", contentClassName)}>
        {children}
      </CardContent>
    </Card>
  );
}

/** A number with its label. */
export function StatTile({
  label,
  value,
  unit,
  detail,
  accent,
}: {
  label: ReactNode;
  value: ReactNode;
  unit?: ReactNode;
  detail?: ReactNode;
  accent?: boolean;
}) {
  return (
    <div
      className={cn(
        "flex min-w-0 flex-col gap-1 rounded-2xl border bg-card p-4 text-left shadow-card",
        accent && "border-primary bg-primary text-primary-foreground",
      )}
    >
      <span
        className={cn(
          "text-ui",
          accent ? "text-primary-foreground" : "text-muted-foreground",
        )}
      >
        {label}
      </span>
      <span className="text-title font-semibold leading-tight tracking-display tabular-nums">
        {value}
        {unit && (
          <span
            className={cn(
              "ml-1 text-sm font-medium tracking-normal",
              accent ? "text-primary-foreground" : "text-muted-foreground",
            )}
          >
            {unit}
          </span>
        )}
      </span>
      {detail && (
        <span
          className={cn(
            "text-xs",
            accent ? "text-primary-foreground" : "text-muted-foreground",
          )}
        >
          {detail}
        </span>
      )}
    </div>
  );
}
