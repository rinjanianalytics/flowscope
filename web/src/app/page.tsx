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
} from "@/components/home/widgets";

export default function HomePage() {
  const config = useServerConfig();
  const topWidgets = config?.homepageTopWidgets ?? [];
  return (
    <div className="flex w-full flex-col gap-6 px-4 py-6 sm:px-6">
      <PageHeader
        eyebrow="Overview"
        title="Your network traffic at a glance"
        description="FlowScope receives flows, adds data like interface names and countries, and stores them in ClickHouse."
        actions={
          <Link href="/visualize" className={buttonVariants()}>
            <ChartArea />
            Explore flows
          </Link>
        }
      />
      <div className="grid grid-cols-1 gap-4 xl:grid-cols-3">
        <div className="grid auto-rows-min grid-cols-2 gap-4 md:grid-cols-4 xl:col-span-2">
          <div className="col-span-2 grid grid-cols-2 gap-4 md:col-span-4 md:grid-cols-4">
            <WidgetFlowRate />
            <WidgetExporters />
          </div>
          {/* The last row grows to fill the width, whatever their number. */}
          <div className="col-span-2 flex flex-wrap gap-4 md:col-span-4">
            {topWidgets.map((widget) => (
              <div key={widget} className="min-w-36 flex-1 basis-36">
                <WidgetTop what={widget} />
              </div>
            ))}
          </div>
          <WidgetGraph className="col-span-2 md:col-span-4" />
        </div>
        <WidgetLastFlow />
      </div>
    </div>
  );
}
