// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import {
  ArrowUpNarrowWide,
  Calendar,
  ChartPie,
  Filter,
  Hash,
  SlidersHorizontal,
  type LucideIcon,
} from "lucide-react";
import { useDocumentTitle } from "@/components/providers";
import { endDate, longDate } from "@/lib/dates";
import { graphTypes, unitLabels } from "@/lib/graph-types";
import type { VisualizeState } from "@/lib/state";
import { cn } from "@/lib/utils";

function Item({
  icon: Icon,
  title,
  className,
  children,
}: {
  icon: LucideIcon;
  title?: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <span
      title={title}
      className={cn(
        "inline-flex min-w-0 items-center gap-1.5 rounded-full bg-secondary px-2.5 py-1 text-ui text-secondary-foreground",
        className,
      )}
    >
      <Icon className="size-3.5 shrink-0 text-muted-foreground" />
      <span className="truncate">{children}</span>
    </span>
  );
}

/** What the displayed result is about. It also sets the page title. */
export function RequestSummary({ request }: { request: VisualizeState | null }) {
  const start = request ? longDate(request.start) : null;
  const end = request ? endDate(request.start, request.end) : null;
  useDocumentTitle(
    request
      ? [
          graphTypes[request.graphType],
          request.dimensions.join(","),
          request.filter,
          start,
          end,
        ]
          .filter((part) => !!part)
          .join(" · ")
      : null,
  );
  if (!request) return null;
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-1.5">
      <Item icon={Calendar} className="shrink-0">
        {start} to {end}
      </Item>
      <Item icon={ChartPie} className="shrink-0">
        {graphTypes[request.graphType]}
      </Item>
      <Item icon={ArrowUpNarrowWide} className="shrink-0" title="Limit">
        {request.limit}
      </Item>
      <Item icon={Hash} className="shrink-0" title="Unit">
        {unitLabels[request.units]}
      </Item>
      {request.dimensions.length > 0 && (
        <Item icon={SlidersHorizontal} title={request.dimensions.join(", ")}>
          {request.dimensions.join(", ")}
        </Item>
      )}
      {request.filter && (
        <Item icon={Filter} title={request.filter} className="max-w-md font-mono">
          {request.filter}
        </Item>
      )}
    </div>
  );
}
