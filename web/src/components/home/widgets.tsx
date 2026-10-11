// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { EChart } from "@/components/echart";
import { useIsDark } from "@/components/providers";
import { Panel, StatTile } from "@/components/brand";
import { Button } from "@/components/ui/button";
import { apiFetch } from "@/lib/api";
import { graphic, type ECOption } from "@/lib/echarts";
import {
  formatSpan,
  formatXps,
  formatXpsLabel,
  groupFlow,
  scaleXps,
} from "@/lib/format";
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

/** The widgets with a few classes sharing the whole: they get a split bar. */
export const isShareWidget = (what: string) =>
  what === "protocol" || what === "etype";

type Slice = { name: string; value: number; color: string };

/** A ranked list: the share of each entry is written out. */
function RankedList({ slices }: { slices: Slice[] }) {
  return (
    <ol className="flex flex-col gap-3">
      {slices.map(({ name, value, color }) => (
        <li key={name} className="flex flex-col gap-1.5">
          <div className="flex items-baseline justify-between gap-3 text-ui">
            <span className="min-w-0 truncate" title={name}>
              {name}
            </span>
            <span className="shrink-0 font-mono text-xs tabular-nums text-muted-foreground">
              {value.toFixed(1)}%
            </span>
          </div>
          <div className="h-1.5 rounded-full bg-muted">
            <div
              className="h-full rounded-full"
              style={{ width: `${value}%`, backgroundColor: color }}
            />
          </div>
        </li>
      ))}
    </ol>
  );
}

/** One bar split between the entries, with their share below. */
function SplitBar({ slices }: { slices: Slice[] }) {
  return (
    <>
      <div
        role="img"
        aria-label={slices
          .map(({ name, value }) => `${name} ${value.toFixed(1)}%`)
          .join(", ")}
        className="flex h-3.5 gap-0.5 overflow-hidden rounded-full"
      >
        {slices.map(({ name, value, color }) => (
          <div
            key={name}
            style={{ width: `${value}%`, backgroundColor: color }}
          />
        ))}
      </div>
      <ul className="flex flex-wrap gap-x-5 gap-y-1 text-ui">
        {slices.map(({ name, value, color }) => (
          <li key={name} className="flex items-center gap-2">
            <span
              className="size-2.5 shrink-0 rounded-sm"
              style={{ backgroundColor: color }}
            />
            {name}
            <span className="font-mono text-xs tabular-nums text-muted-foreground">
              {value.toFixed(1)}%
            </span>
          </li>
        ))}
      </ul>
    </>
  );
}

export function WidgetTop({ what }: { what: string }) {
  const isDark = useIsDark();
  const data = useWidget<{ top: { name: string; percent: number }[] }>(
    `top/${what}`,
    refreshOccasionally,
  );
  const theme = isDark ? "dark" : "light";
  const share = isShareWidget(what);
  const slices = useMemo((): Slice[] => {
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
        // A ranked list has one color: the length of the bar says it all.
        color:
          slice.name === "Others"
            ? dataColorGrey(0, false, theme)
            : dataColor(share ? index : 0, false, theme),
      }));
  }, [data, share, theme]);
  return (
    <Panel size="sm" title={widgetTitles[what] ?? "???"} className="h-full">
      {share ? <SplitBar slices={slices} /> : <RankedList slices={slices} />}
    </Panel>
  );
}

/** The points of the graph of the home page, without the one being filled. */
function useTraffic() {
  const data = useWidget<{ data: { t: string; gbps: number }[] }>(
    "graph",
    refreshInfrequently,
  );
  return useMemo(() => {
    if (!data) return null;
    const points = data.data ?? [];
    const first = points[0];
    const last = points[points.length - 1];
    return {
      points: points.slice(0, -1),
      span:
        first && last
          ? new Date(last.t).valueOf() - new Date(first.t).valueOf()
          : 0,
    };
  }, [data]);
}

function TrafficTile({
  label,
  gbps,
}: {
  label: string;
  gbps: number | undefined;
}) {
  if (gbps === undefined) return <StatTile label={label} value="???" />;
  const { value, suffix } = scaleXps(gbps * 1_000_000_000);
  return (
    <StatTile label={label} value={value.toFixed(1)} unit={`${suffix}bps`} />
  );
}

export function WidgetTrafficNow() {
  const traffic = useTraffic();
  return (
    <TrafficTile label="Traffic now" gbps={traffic?.points.at(-1)?.gbps} />
  );
}

export function WidgetTrafficPeak() {
  const traffic = useTraffic();
  return (
    <TrafficTile
      label={traffic?.span ? `Peak, last ${formatSpan(traffic.span)}` : "Peak"}
      gbps={
        traffic?.points.length
          ? Math.max(...traffic.points.map(({ gbps }) => gbps))
          : undefined
      }
    />
  );
}

const formatGbps = (value: number) =>
  formatXps(value * 1_000_000_000) + "bps";

export function WidgetGraph({ className }: { className?: string }) {
  const isDark = useIsDark();
  const traffic = useTraffic();
  const option = useMemo((): ECOption => {
    const theme = isDark ? "dark" : "light";
    const chrome = chartChrome(theme);
    return {
      darkMode: isDark,
      backgroundColor: "transparent",
      grid: { left: 76, top: 20, right: 20, bottom: 30 },
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
        axisLabel: {
          formatter: (value: number) =>
            formatXpsLabel(value * 1_000_000_000, "bps"),
          color: chrome.text,
        },
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
          data: (traffic?.points ?? []).map(({ t, gbps }) => [t, gbps]),
        },
      ],
    };
  }, [traffic, isDark]);
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

export function WidgetLastFlow() {
  const data = useWidget<Record<string, unknown>>("flow-last", refreshOften);
  const [showEmpty, setShowEmpty] = useState(false);
  const { sections, empty } = useMemo(() => groupFlow(data ?? {}), [data]);
  return (
    <Panel
      eyebrow="Live"
      title="Last flow"
      description="The most recent flow stored in ClickHouse"
    >
      {sections.map(({ title, fields }) => (
        <section key={title} className="flex flex-col gap-1">
          <h3 className="text-2xs font-semibold uppercase tracking-label text-muted-foreground">
            {title}
          </h3>
          <dl className="text-ui">
            {fields.map(([field, value]) => (
              <div
                key={field}
                className="flex justify-between gap-4 border-b border-border/50 py-1 last:border-0"
              >
                <dt className="shrink-0 text-muted-foreground">{field}</dt>
                <dd className="min-w-0 text-right font-mono text-xs leading-5 wrap-anywhere">
                  {value}
                </dd>
              </div>
            ))}
          </dl>
        </section>
      ))}
      {showEmpty && (
        <section className="flex flex-col gap-2">
          <h3 className="text-2xs font-semibold uppercase tracking-label text-muted-foreground">
            Empty in this flow
          </h3>
          <ul className="flex flex-wrap gap-1.5">
            {empty.map((field) => (
              <li
                key={field}
                className="rounded-md bg-muted px-2 py-0.5 font-mono text-xs text-muted-foreground"
              >
                {field}
              </li>
            ))}
          </ul>
        </section>
      )}
      {empty.length > 0 && (
        <Button
          variant="outline"
          aria-expanded={showEmpty}
          onClick={() => setShowEmpty(!showEmpty)}
          className="h-10 self-start bg-card px-4"
        >
          {showEmpty
            ? "Hide empty fields"
            : `Show ${empty.length} empty ${empty.length === 1 ? "field" : "fields"}`}
        </Button>
      )}
    </Panel>
  );
}
