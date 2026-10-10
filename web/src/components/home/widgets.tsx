// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { EChart } from "@/components/echart";
import { useIsDark } from "@/components/providers";
import { Panel, StatTile } from "@/components/brand";
import { apiFetch } from "@/lib/api";
import { graphic, type ECOption } from "@/lib/echarts";
import { compareFields, formatXps } from "@/lib/format";
import { chartChrome, dataColor, dataColorGrey } from "@/lib/palette";

const refreshOften = 10_000;
const refreshOccasionally = 60_000;
const refreshInfrequently = 600_000;

/** Fetch a widget, again at a regular interval. */
function useWidget<T>(name: string, interval: number) {
  return useQuery({
    queryKey: ["widget", name],
    queryFn: () => apiFetch<T>(`widget/${name}`),
    refetchInterval: interval,
  }).data;
}

export function WidgetFlowRate() {
  const data = useWidget<{ rate: number; period: string }>(
    "flow-rate",
    refreshOften,
  );
  let rate = "???";
  if (data) {
    if (data.rate > 1_500_000) rate = (data.rate / 1_000_000).toFixed(1) + "M";
    else if (data.rate > 1_500) rate = (data.rate / 1_000).toFixed(1) + "K";
    else rate = data.rate.toFixed(0);
  }
  return <StatTile label="Flow rate" value={rate} unit="flows/s" />;
}

export function WidgetExporters() {
  const data = useWidget<{ exporters: string[] }>(
    "exporters",
    refreshOccasionally,
  );
  return (
    <StatTile
      label="Exporters"
      value={data ? data.exporters.length.toString() : "???"}
      detail="sending flows"
    />
  );
}

const widgetTitles: Record<string, string> = {
  "src-as": "Top source AS",
  "dst-as": "Top destination AS",
  "src-country": "Top source countries",
  "dst-country": "Top destination countries",
  exporter: "Top exporters",
  protocol: "Top protocols",
  etype: "IPv4/IPv6",
  "src-port": "Top source ports",
  "dst-port": "Top destination ports",
};

export function WidgetTop({ what }: { what: string }) {
  const isDark = useIsDark();
  const data = useWidget<{ top: { name: string; percent: number }[] }>(
    `top/${what}`,
    refreshOccasionally,
  );
  const theme = isDark ? "dark" : "light";
  const slices = useMemo(() => {
    if (!data) return [];
    const top = data.top ?? [];
    return [
      ...top
        .filter(({ percent }) => percent > 0)
        .map(({ name, percent }) => ({ name, value: percent })),
      {
        name: "Others",
        value: Math.max(100 - top.reduce((c, n) => c + n.percent, 0), 0),
      },
    ]
      .filter(({ value }) => value > 0.05)
      .map((slice, index) => ({
        ...slice,
        color:
          slice.name === "Others"
            ? dataColorGrey(0, false, theme)
            : dataColor(index, false, theme),
      }));
  }, [data, theme]);
  const option = useMemo(
    (): ECOption => ({
      darkMode: isDark,
      backgroundColor: "transparent",
      tooltip: {
        trigger: "item",
        confine: true,
        valueFormatter: (value) =>
          ((value?.valueOf() as number) ?? 0).toFixed(2) + "%",
      },
      series: [
        {
          type: "pie",
          label: { show: false },
          radius: "90%",
          data: slices.map(({ name, value, color }) => ({
            name,
            value,
            itemStyle: { color },
          })),
        },
      ],
    }),
    [slices, isDark],
  );
  // The legend is not drawn by the chart: it wraps on narrow tiles without
  // covering the pie.
  return (
    <Panel title={widgetTitles[what] ?? "???"} className="h-full">
      <div className="h-[130px]">
        <EChart option={option} />
      </div>
      <ul className="flex flex-wrap justify-center gap-x-2.5 gap-y-1 text-2xs text-muted-foreground">
        {slices.map(({ name, color }) => (
          <li key={name} title={name} className="flex items-center gap-1">
            <span
              className="size-2.5 shrink-0 rounded-sm"
              style={{ backgroundColor: color }}
            />
            {name.split(": ")[0]}
          </li>
        ))}
      </ul>
    </Panel>
  );
}

const formatGbps = (value: number) => formatXps(value * 1_000_000_000);

export function WidgetGraph({ className }: { className?: string }) {
  const isDark = useIsDark();
  const data = useWidget<{ data: { t: string; gbps: number }[] }>(
    "graph",
    refreshInfrequently,
  );
  const option = useMemo((): ECOption => {
    const theme = isDark ? "dark" : "light";
    const chrome = chartChrome(theme);
    return {
      darkMode: isDark,
      backgroundColor: "transparent",
      grid: { left: 60, top: 20, right: 20, bottom: 30 },
      textStyle: { fontFamily: "Poppins, sans-serif" },
      xAxis: {
        type: "time",
        axisLine: { lineStyle: { color: chrome.axis } },
        axisLabel: { color: chrome.text },
        splitLine: { show: false },
      },
      yAxis: {
        type: "value",
        min: 0,
        axisLabel: { formatter: formatGbps, color: chrome.text },
        splitLine: { lineStyle: { color: chrome.grid } },
      },
      tooltip: {
        confine: true,
        trigger: "axis",
        axisPointer: { type: "cross", label: { backgroundColor: "#6a7985" } },
        valueFormatter: (value) =>
          formatGbps((value?.valueOf() as number) ?? 0),
      },
      series: [
        {
          type: "line",
          symbol: "none",
          lineStyle: { width: 2, color: dataColor(0, false, theme) },
          areaStyle: {
            color: new graphic.LinearGradient(0, 0, 0, 1, [
              { offset: 0, color: dataColor(0, false, theme) + "66" },
              { offset: 1, color: dataColor(0, false, theme) + "08" },
            ]),
          },
          // The last point is still being filled.
          data: (data?.data ?? []).map(({ t, gbps }) => [t, gbps]).slice(0, -1),
        },
      ],
    };
  }, [data, isDark]);
  return (
    <Panel
      eyebrow="Traffic"
      title="Total traffic"
      description="All flows matching the home page filter"
      className={className}
      contentClassName="h-[300px]"
    >
      <EChart option={option} />
    </Panel>
  );
}

const flowValue = (value: unknown) =>
  Array.isArray(value) ? value.join(" ") : String(value);

export function WidgetLastFlow() {
  const data = useWidget<Record<string, unknown>>("flow-last", refreshOften);
  const fields = Object.entries(data ?? {}).sort(([f1], [f2]) =>
    compareFields(f1, f2),
  );
  return (
    <Panel
      eyebrow="Live"
      title="Last flow"
      description="The most recent flow stored in ClickHouse"
    >
        <table className="w-full table-fixed text-ui">
          <tbody>
            {fields.map(([field, value]) => (
              <tr key={field} className="border-b border-border/50 last:border-0">
                <td className="w-2/5 truncate py-0.5 pr-3 align-top text-muted-foreground">
                  {field}
                </td>
                <td className="w-3/5 truncate py-0.5 align-top font-mono text-xs leading-5">
                  {flowValue(value)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
    </Panel>
  );
}
