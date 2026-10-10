// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { useCallback, useEffect, useMemo, useRef } from "react";
import { EChart } from "@/components/echart";
import { useIsDark } from "@/components/providers";
import type { GraphLineResult, GraphResult, GraphSankeyResult } from "@/lib/api";
import {
  graphic,
  type ECharts,
  type ECOption,
  type LineSeriesOption,
  type SankeySeriesOption,
} from "@/lib/echarts";
import { formatXps, rowName, sameRow, uniqueRows } from "@/lib/format";
import { chartChrome, dataColor, dataColorGrey } from "@/lib/palette";

type TimeRangeHandler = (range: [Date, Date]) => void;

const commonGraph: ECOption = {
  backgroundColor: "transparent",
  animationDuration: 500,
  textStyle: { fontFamily: "Poppins, sans-serif" },
  toolbox: { show: false },
  brush: { xAxisIndex: "all" },
};

/**
 * A graph with time on its X axis. A time range can be selected with the
 * mouse, and a series can be highlighted.
 */
function TimeSeriesGraph({
  graph,
  highlight,
  onTimeRange,
  coordRangeIndex = 0,
}: {
  graph: ECOption;
  highlight: number | null;
  onTimeRange: TimeRangeHandler;
  coordRangeIndex?: number;
}) {
  const chart = useRef<ECharts | null>(null);
  const option = useMemo(() => ({ ...commonGraph, ...graph }), [graph]);

  // Selecting is the default action of the mouse, except on touch screens
  // where it would prevent scrolling.
  const enableBrush = useCallback((instance: ECharts) => {
    chart.current = instance;
    const isTouchScreen = window.matchMedia("(pointer: coarse)").matches;
    instance.dispatchAction({
      type: "takeGlobalCursor",
      key: "brush",
      brushOption: { brushType: isTouchScreen ? false : "lineX" },
    });
  }, []);

  const onBrushEnd = useCallback(
    (params: unknown, instance: ECharts) => {
      const areas = (
        params as { areas?: { coordRanges?: (number | string)[][] }[] }
      ).areas;
      const coordRange = areas?.[0]?.coordRanges?.[coordRangeIndex];
      if (!coordRange) return;
      const [start, end] = coordRange.map((t) => new Date(t as number));
      instance.dispatchAction({ type: "brush", areas: [] });
      onTimeRange([start, end]);
    },
    [coordRangeIndex, onTimeRange],
  );

  useEffect(() => {
    const instance = chart.current;
    if (!instance || instance.isDisposed()) return;
    instance.dispatchAction({ type: "downplay" });
    if (highlight !== null) {
      instance.dispatchAction({ type: "highlight", seriesIndex: highlight });
    }
  }, [highlight, option]);

  return <EChart option={option} onChart={enableBrush} onBrushEnd={onBrushEnd} />;
}

function useLineOption(data: GraphLineResult, isDark: boolean): ECOption {
  return useMemo(() => {
    const theme = isDark ? "dark" : "light";
    const chrome = chartChrome(theme);
    const percent = ["inl2%", "outl2%"].includes(data.units);
    const source: [string, ...number[]][] = data.t
      .map((t, timeIdx) => {
        let result: [string, ...number[]] = [
          t,
          ...data.points.map(
            // ECharts does not make it easy to inverse an axis and put the
            // result below, so the second axis uses negative values.
            (row, rowIdx) => row[timeIdx] * (data.axis[rowIdx] % 2 ? 1 : -1),
          ),
        ];
        if (data.graphType === "stacked100") {
          // Normalize values between 0 and 1 (or -1 and 0)
          const [, ...values] = result;
          const positiveSum = values.reduce((p, c) => (c > 0 ? p + c : p), 0);
          const negativeSum = values.reduce((p, c) => (c < 0 ? p + c : p), 0);
          result = [
            t,
            ...values.map((v) =>
              v > 0 && positiveSum > 0
                ? v / positiveSum
                : v < 0 && negativeSum < 0
                  ? -v / negativeSum
                  : v,
            ),
          ];
        }
        return result;
      })
      .slice(0, -1); // trim last point
    const dataset = {
      sourceHeader: false,
      dimensions: ["time", ...data.rows.map(rowName)],
      source,
    };
    const xAxis = {
      type: "time" as const,
      min: data.start,
      max: data.end,
      axisLine: { lineStyle: { color: chrome.axis } },
      axisLabel: { color: chrome.text },
      splitLine: { show: false },
    };
    const yAxis = {
      type: "value" as const,
      min: data.bidirectional
        ? data.graphType === "stacked100"
          ? -1
          : undefined
        : 0,
      max: data.graphType === "stacked100" ? 1 : undefined,
      splitLine: { lineStyle: { color: chrome.grid } },
      axisLabel: {
        color: chrome.text,
        formatter:
          data.graphType === "stacked100"
            ? (v: number) => (v * 100).toFixed(0)
            : percent
              ? (v: number) => v.toFixed(0)
              : formatXps,
      },
      axisPointer: {
        label: {
          formatter: ({ value }: { value: unknown }) => {
            const v = Number(value);
            return data.graphType === "stacked100"
              ? (v * 100).toFixed(1)
              : percent
                ? v.toFixed(0)
                : formatXps(v);
          },
        },
      },
    };
    const tooltip: ECOption["tooltip"] = {
      confine: true,
      trigger: "axis",
      axisPointer: { type: "cross", label: { backgroundColor: "#5a6270" } },
      backgroundColor: isDark ? "#1b2025ee" : "#ffffffee",
      borderColor: chrome.grid,
      textStyle: { color: chrome.ink, fontSize: 12 },
      formatter: (params) => {
        // A custom formatter, notably to handle bidirectional tooltips.
        if (!Array.isArray(params) || params.length === 0) return "";
        const table: {
          key: string;
          seriesName: string;
          marker: string;
          up: number;
          down: number;
        }[] = [];
        for (const param of params) {
          if (param.seriesIndex === undefined) continue;
          const axis = data.axis[param.seriesIndex];
          const seriesName = [1, 2].includes(axis)
            ? param.seriesName
            : data["axis-names"][axis];
          if (!seriesName) continue;
          const key = `${Math.floor((axis - 1) / 2)}-${seriesName}`;
          let row = table.find((r) => r.key === key);
          if (!row) {
            row = {
              key,
              seriesName,
              marker: String(param.marker ?? ""),
              up: 0,
              down: 0,
            };
            table.push(row);
          }
          // The original value is in data.points, notably with stacked100.
          const val = data.points[param.seriesIndex][param.dataIndex];
          if (axis % 2 == 1) row.up = val;
          else row.down = val;
        }
        const rows = table
          .map((row) =>
            [
              `<tr>`,
              `<td>${row.marker} ${row.seriesName}</td>`,
              `<td style="padding-left:0.5rem">${data.bidirectional ? "↑" : ""}<b>${formatXps(row.up)}</b></td>`,
              data.bidirectional
                ? `<td style="padding-left:0.5rem">↓<b>${formatXps(row.down)}</b></td>`
                : "",
              `</tr>`,
            ].join(""),
          )
          .join("");
        const label = (params[0] as { axisValueLabel?: string }).axisValueLabel;
        return `${label}<table>${rows}</table>`;
      },
    };

    // Lines and stacked areas
    if (data.graphType !== "grid") {
      const uniq = uniqueRows(data.rows);
      const uniqIndex = (row: string[]) => uniq.findIndex((o) => sameRow(row, o));
      return {
        grid: { left: 60, top: 20, right: "1%", bottom: 24 },
        xAxis,
        yAxis,
        dataset,
        tooltip,
        series: data.rows
          .map((row, idx) => {
            const isOther = row.some((name) => name === "Other");
            const color = isOther ? dataColorGrey : dataColor;
            if (data.graphType === "lines" && isOther) return undefined;
            let serie: LineSeriesOption = {
              type: "line",
              symbol: "none",
              itemStyle: { color: color(uniqIndex(row), false, theme) },
              lineStyle: { color: color(uniqIndex(row), false, theme), width: 2 },
              emphasis: { focus: "series" },
              encode: {
                x: 0,
                y: idx + 1,
                seriesName: idx + 1,
                seriesId: idx + 1,
              },
            };
            if ([3, 4].includes(data.axis[idx])) {
              // Previous period
              serie = {
                ...serie,
                itemStyle: { color: dataColorGrey(1, false, theme) },
                lineStyle: {
                  color: chrome.text,
                  width: 2,
                  type: "dashed",
                },
              };
            }
            if (
              (data.graphType === "stacked" ||
                data.graphType === "stacked100") &&
              [1, 2].includes(data.axis[idx])
            ) {
              const last =
                idx == data.rows.length - 1 ||
                data.axis[idx] != data.axis[idx + 1];
              serie = {
                ...serie,
                stack: data.axis[idx].toString(),
                lineStyle: last
                  ? { color: chrome.ink, width: 1.5 }
                  : { color: color(uniqIndex(row), false, theme), width: 1 },
                areaStyle: {
                  opacity: 0.95,
                  color: new graphic.LinearGradient(0, 0, 0, 1, [
                    { offset: 0, color: color(uniqIndex(row), false, theme) },
                    { offset: 1, color: color(uniqIndex(row), true, theme) },
                  ]),
                },
              };
            }
            return serie;
          })
          .filter((s): s is LineSeriesOption => !!s),
      };
    }

    // Grid: one small graph for each row
    const uniq = uniqueRows(data.rows).filter((row) =>
      row.some((name) => name !== "Other"),
    );
    const uniqIndex = (row: string[]) => uniq.findIndex((o) => sameRow(row, o));
    const otherIndexes = data.rows
      .map((row, idx) => (row.some((name) => name === "Other") ? idx : -1))
      .filter((idx) => idx >= 0);
    const somethingY = (fn: (...n: number[]) => number) =>
      fn(
        ...source.map((row) => {
          const [, ...cdr] = row;
          return fn(...cdr.filter((_, idx) => !otherIndexes.includes(idx)));
        }),
      );
    const maxY = somethingY(Math.max);
    const minY = somethingY(Math.min);
    let rowNumber = Math.ceil(Math.sqrt(uniq.length));
    const colNumber = rowNumber;
    if ((rowNumber - 1) * colNumber >= uniq.length) {
      rowNumber--;
    }
    const positions = uniq.map((_, idx) => ({
      left: ((idx % colNumber) / colNumber) * 100,
      top: (Math.floor(idx / colNumber) / rowNumber) * 100,
      width: (1 / colNumber) * 100,
      height: (1 / rowNumber) * 100,
    }));
    return {
      title: uniq.map((row, idx) => ({
        textAlign: "left",
        textStyle: {
          fontSize: 12,
          fontWeight: 600,
          color: chrome.ink,
          textBorderWidth: 2,
          textBorderColor: isDark ? "#0f1215aa" : "#ffffffaa",
        },
        text: rowName(row),
        bottom: 100 - positions[idx].top - positions[idx].height - 0.5 + "%",
        left: positions[idx].left + 0.25 + "%",
      })),
      grid: uniq.map((_, idx) => ({
        show: true,
        borderWidth: 0,
        left: positions[idx].left + 0.25 + "%",
        top: positions[idx].top + 0.25 + "%",
        width: positions[idx].width - 0.5 + "%",
        height: positions[idx].height - 0.5 + "%",
      })),
      xAxis: uniq.map((_, idx) => ({ ...xAxis, gridIndex: idx, show: false })),
      yAxis: uniq.map((_, idx) => ({
        ...yAxis,
        max: maxY,
        min: data.bidirectional ? minY : 0,
        gridIndex: idx,
        show: false,
      })),
      dataset,
      series: data.rows
        .map((row, idx): LineSeriesOption => {
          const index = uniqIndex(row);
          return {
            type: "line",
            symbol: "none",
            xAxisIndex: index,
            yAxisIndex: index,
            itemStyle: { color: dataColor(index, false, theme) },
            areaStyle: {
              opacity: 0.95,
              color: new graphic.LinearGradient(0, 0, 0, 1, [
                { offset: 0, color: dataColor(index, false, theme) },
                { offset: 1, color: dataColor(index, true, theme) },
              ]),
            },
            emphasis: { focus: "series" },
            encode: {
              x: 0,
              y: idx + 1,
              seriesName: idx + 1,
              seriesId: idx + 1,
            },
          };
        })
        .filter((s) => (s.xAxisIndex as number) >= 0),
    };
  }, [data, isDark]);
}

function LineGraph({
  data,
  highlight,
  onTimeRange,
}: {
  data: GraphLineResult;
  highlight: number | null;
  onTimeRange: TimeRangeHandler;
}) {
  const graph = useLineOption(data, useIsDark());
  return (
    <TimeSeriesGraph graph={graph} highlight={highlight} onTimeRange={onTimeRange} />
  );
}

// From light to dark, in the hue of the brand.
const heatScale = ["#e3efec", "#a8ddd3", "#509087", "#23685e", "#0f2f2a"];

const compareNames = (a: string[], b: string[]) =>
  rowName(a).localeCompare(rowName(b), "en", { numeric: true });

function HeatmapGraph({
  data,
  highlight,
  onTimeRange,
}: {
  data: GraphLineResult;
  highlight: number | null;
  onTimeRange: TimeRangeHandler;
}) {
  const isDark = useIsDark();
  const graph = useMemo((): ECOption => {
    const chrome = chartChrome(isDark ? "dark" : "light");
    const named = data.rows
      .map((row, idx) => ({ row, idx }))
      .filter(({ row }) => row.some((name) => name !== "Other"))
      .sort((a, b) => compareNames(a.row, b.row));
    const source = named.flatMap(({ idx }, rowIdx) =>
      data.t.flatMap((t, timeIdx): [string, number, number][] => {
        const value = data.points[idx][timeIdx] * (data.axis[idx] % 2 ? 1 : -1);
        return value === 0 ? [] : [[t, rowIdx, value]];
      }),
    );
    return {
      grid: { left: 150, top: 20, right: "1%", bottom: 80 },
      xAxis: [
        {
          type: "category",
          data: data.t,
          show: false,
          axisPointer: { show: false },
        },
        {
          type: "time",
          min: data.start,
          max: data.end,
          position: "bottom",
          axisLine: { lineStyle: { color: chrome.axis } },
          axisLabel: { color: chrome.text },
        },
      ],
      yAxis: {
        type: "category",
        data: named.map(({ row }) => rowName(row)),
        axisLine: { lineStyle: { color: chrome.axis } },
        axisLabel: { color: chrome.text },
      },
      visualMap: {
        type: "continuous",
        min: 0,
        max: Math.max(0, ...named.map(({ idx }) => data.max[idx])),
        calculable: true,
        orient: "horizontal",
        right: "5%",
        bottom: 0,
        textStyle: { color: chrome.text },
        inRange: { color: isDark ? [...heatScale].reverse() : heatScale },
        formatter: (value) => formatXps(value as number),
      },
      tooltip: {
        confine: true,
        trigger: "axis",
        axisPointer: { type: "cross", label: { backgroundColor: "#5a6270" } },
        formatter: () => "",
      },
      series: [{ type: "heatmap", data: source }],
    };
  }, [data, isDark]);
  return (
    <TimeSeriesGraph
      graph={graph}
      highlight={highlight}
      onTimeRange={onTimeRange}
      coordRangeIndex={1}
    />
  );
}

function SankeyGraph({ data }: { data: GraphSankeyResult }) {
  const isDark = useIsDark();
  const option = useMemo((): ECOption => {
    const theme = isDark ? "dark" : "light";
    const chrome = chartChrome(theme);
    if (!data.xps) return {};
    // Allocate colors by node value (the part after ": ") so the same value
    // gets the same color across the forward and reverse halves.
    const colorByValue = new Map<string, number>();
    const greyByValue = new Map<string, number>();
    const label = (name: string) => name.split(": ").slice(1).join(": ");
    const makeNode = ({ name }: GraphSankeyResult["nodes"][0]) => {
      const isGrey = name.endsWith(" Other");
      const cache = isGrey ? greyByValue : colorByValue;
      let idx = cache.get(label(name));
      if (idx === undefined) {
        idx = cache.size;
        cache.set(label(name), idx);
      }
      return {
        id: name,
        name: label(name),
        itemStyle: {
          color: isGrey
            ? dataColorGrey(idx, false, theme)
            : dataColor(idx, false, theme),
        },
      };
    };
    const makeLink = ({ source, target, xps }: GraphSankeyResult["links"][0]) => ({
      source,
      target,
      value: xps,
    });
    const seriesBase = {
      type: "sankey" as const,
      animationDuration: 500,
      emphasis: { focus: "trajectory" as const },
      label: { formatter: "{b}", color: chrome.ink, fontFamily: "Poppins" },
      lineStyle: { color: "gradient" as const, curveness: 0.5 },
    };
    const bold = (value: number) =>
      `<span style="display:inline-block;margin-left:2em;font-weight:bold;">${formatXps(value)}</span>`;
    const tooltip: ECOption["tooltip"] = {
      confine: true,
      trigger: "item",
      triggerOn: "mousemove",
      formatter(params) {
        if (Array.isArray(params)) return "";
        const { dataType, marker, data: item, value } = params as {
          dataType?: string;
          marker?: string;
          data: { name?: string; source?: string; target?: string };
          value?: number;
        };
        if (dataType === "node") {
          return `${marker}<span style="display:inline-block;margin-left:1em;">${item.name}</span>${bold(value ?? 0)}`;
        }
        if (dataType === "edge" && value) {
          return `${label(item.source ?? "") || "???"} → ${label(item.target ?? "") || "???"}${bold(value)}`;
        }
        return "";
      },
    };
    // Put the labels of the last column of nodes on their left. The nodes on
    // the last column are not the source of a link.
    const positionLabels = <T extends { id: string }>(
      nodes: T[],
      links: { source: string }[],
    ) => {
      const sources = new Set(links.map((l) => l.source));
      return nodes.map((n) =>
        sources.has(n.id) ? n : { ...n, label: { position: "left" as const } },
      );
    };
    let series: SankeySeriesOption[];
    if (data.bidirectional) {
      const fwdLinks = data.links.filter((l) => l.axis === 1).map(makeLink);
      // Flip source and target on the reverse half so the column order is
      // mirrored: the dimension closest to the forward side appears first.
      const revLinks = data.links
        .filter((l) => l.axis === 2)
        .map(makeLink)
        .map((l) => ({ ...l, source: l.target, target: l.source }));
      series = [
        {
          ...seriesBase,
          left: "1%",
          right: "50%",
          data: positionLabels(
            data.nodes.filter((n) => n.axis === 1).map(makeNode),
            fwdLinks,
          ),
          links: fwdLinks,
        },
        {
          ...seriesBase,
          left: "50%",
          right: "1%",
          data: positionLabels(
            data.nodes.filter((n) => n.axis === 2).map(makeNode),
            revLinks,
          ),
          links: revLinks,
        },
      ];
    } else {
      const links = data.links.map(makeLink);
      series = [
        {
          ...seriesBase,
          left: "1%",
          right: "1%",
          data: positionLabels(data.nodes.map(makeNode), links),
          links,
        },
      ];
    }
    return { backgroundColor: "transparent", tooltip, series };
  }, [data, isDark]);
  return <EChart option={option} />;
}

/** The graph matching the type of the result. */
export function DataGraph({
  data,
  highlight,
  onTimeRange,
}: {
  data: GraphResult | null;
  highlight: number | null;
  onTimeRange: TimeRangeHandler;
}) {
  if (!data) return <div />;
  switch (data.graphType) {
    case "sankey":
      return <SankeyGraph data={data} />;
    case "heatmap":
      return (
        <HeatmapGraph data={data} highlight={highlight} onTimeRange={onTimeRange} />
      );
    default:
      return (
        <LineGraph data={data} highlight={highlight} onTimeRange={onTimeRange} />
      );
  }
}
