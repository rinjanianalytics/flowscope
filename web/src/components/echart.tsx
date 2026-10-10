// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { useEffect, useRef } from "react";
import { init, type ECharts, type ECOption } from "@/lib/echarts";
import { useIsDark } from "@/components/providers";
import { cn } from "@/lib/utils";

/** An ECharts graph filling its container. */
export function EChart({
  option,
  className,
  onBrushEnd,
  onChart,
}: {
  option: ECOption;
  className?: string;
  /** Called with the brush event when a selection ends. */
  onBrushEnd?: (params: unknown, chart: ECharts) => void;
  /** Called after each update, with the chart. */
  onChart?: (chart: ECharts) => void;
}) {
  const isDark = useIsDark();
  const element = useRef<HTMLDivElement>(null);
  const chart = useRef<ECharts | null>(null);
  const handlers = useRef({ onBrushEnd, onChart });
  useEffect(() => {
    handlers.current = { onBrushEnd, onChart };
  });

  // The theme is set when creating the chart.
  useEffect(() => {
    if (!element.current) return;
    const instance = init(element.current, isDark ? "dark" : undefined);
    chart.current = instance;
    instance.on("brushEnd", (params) =>
      handlers.current.onBrushEnd?.(params, instance),
    );
    const observer = new ResizeObserver(() => instance.resize());
    observer.observe(element.current);
    return () => {
      observer.disconnect();
      instance.dispose();
      chart.current = null;
    };
  }, [isDark]);

  useEffect(() => {
    if (!chart.current) return;
    chart.current.setOption(option, { notMerge: true });
    handlers.current.onChart?.(chart.current);
  }, [option, isDark]);

  return <div ref={element} className={cn("size-full", className)} />;
}
