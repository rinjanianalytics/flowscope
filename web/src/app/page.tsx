// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import Link from "next/link";
import { ChartArea } from "lucide-react";
import { PageHeader } from "@/components/brand";
import { buttonVariants } from "@/components/ui/button";
import { useServerConfig } from "@/components/providers";
import {
  WidgetExporters,
  WidgetFlowRate,
  WidgetGraph,
  WidgetLastFlow,
  WidgetTop,
  WidgetTrafficNow,
  WidgetTrafficPeak,
  isShareWidget,
} from "@/components/home/widgets";

export default function HomePage() {
  const config = useServerConfig();
  const topWidgets = config?.homepageTopWidgets ?? [];
  const ranked = topWidgets.filter((widget) => !isShareWidget(widget));
  const shares = topWidgets.filter(isShareWidget);
  return (
    <div className="flex w-full flex-col gap-6 px-4 py-6 sm:px-6">
      <PageHeader
        eyebrow="Overview"
        title="Your network traffic at a glance"
        description="FlowScope receives flows, adds data like interface names and countries, and stores them in ClickHouse."
        actions={
          <Link
            href="/visualize"
            className={buttonVariants({ className: "h-11 px-5" })}
          >
            <ChartArea />
            Explore flows
          </Link>
        }
      />
      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <WidgetFlowRate />
        <WidgetExporters />
        <WidgetTrafficNow />
        <WidgetTrafficPeak />
      </div>
      <div className="grid grid-cols-1 items-start gap-4 xl:grid-cols-3">
        <div className="flex min-w-0 flex-col gap-4 xl:col-span-2">
          <WidgetGraph />
          {/* Each row fills the width, whatever the number of widgets. */}
          {[ranked, shares].map(
            (widgets, row) =>
              widgets.length > 0 && (
                <div
                  key={row}
                  className="grid grid-cols-[repeat(auto-fit,minmax(13.5rem,1fr))] gap-4"
                >
                  {widgets.map((widget) => (
                    <WidgetTop key={widget} what={widget} />
                  ))}
                </div>
              ),
          )}
        </div>
        <WidgetLastFlow />
      </div>
    </div>
  );
}
