// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { useMemo, useState } from "react";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useIsDark, useServerConfig } from "@/components/providers";
import type { GraphResult } from "@/lib/api";
import {
  formatXps,
  reverseDimension,
  sameRow,
  uniqueRows,
} from "@/lib/format";
import { dataColor, dataColorGrey } from "@/lib/palette";
import { cn } from "@/lib/utils";

type Cell = { value: string; numeric?: boolean };

/** The rows of a result, with their statistics. */
export function DataTable({
  data,
  onHighlight,
}: {
  data: GraphResult | null;
  /** Called with the index of the row under the pointer. */
  onHighlight: (index: number | null) => void;
}) {
  const theme = useIsDark() ? "dark" : "light";
  const config = useServerConfig();
  const [selectedAxis, setSelectedAxis] = useState(1);

  const axes = useMemo(
    () =>
      Object.entries(data?.["axis-names"] ?? {})
        .map(([k, v]) => ({ id: Number(k), name: v }))
        .filter(({ id }) => [1, 2].includes(id))
        .sort((a, b) => a.id - b.id),
    [data],
  );
  const displayedAxis = axes.some((axis) => axis.id === selectedAxis)
    ? selectedAxis
    : 1;

  const table = useMemo(() => {
    if (!data) return null;
    // The reverse axis groups on the opposite dimensions.
    const known = config?.dimensions ?? [];
    const dimensions =
      displayedAxis === 2
        ? data.dimensions.map((d) => reverseDimension(d, known))
        : data.dimensions;
    const unit = ["inl2%", "outl2%"].includes(data.units)
      ? "%"
      : data.units.slice(-3);
    const formatValue = (v: number) =>
      unit === "%" ? `${v.toFixed(0)}%` : `${formatXps(v)}${unit}`;
    const isBps = data.units.endsWith("bps");
    const totalUnit = isBps ? "B" : data.units.endsWith("pps") ? "p" : "f";
    const formatTotal = (v: number) =>
      `${formatXps(isBps ? v / 8 : v)}${totalUnit}`;
    const names = dimensions.map((col) => col.replace(/([a-z])([A-Z])/g, "$1 $2"));
    const rows: { index: number; cells: Cell[]; color?: string }[] = [];
    if (data.graphType === "sankey") {
      data.rows.forEach((row, idx) => {
        if (data.axis[idx] !== displayedAxis) return;
        rows.push({
          index: idx,
          cells: [
            ...row.map((value) => ({ value })),
            { value: formatValue(data.xps[idx]), numeric: true },
          ],
        });
      });
      return { columns: [...names, "Average"], numeric: 1, rows };
    }
    const uniq = uniqueRows(data.rows);
    const stats = ["Min", "Max", "Last", "Average", "~95th"];
    if (data.total) stats.push("Total");
    data.rows.forEach((row, idx) => {
      if (data.axis[idx] !== displayedAxis) return;
      const color = row.some((name) => name === "Other")
        ? dataColorGrey
        : dataColor;
      rows.push({
        index: idx,
        cells: [
          ...row.map((value) => ({ value })),
          ...[
            data.min[idx],
            data.max[idx],
            data.last[idx],
            data.average[idx],
            data["95th"][idx],
          ].map((value) => ({ value: formatValue(value), numeric: true })),
          ...(data.total
            ? [{ value: formatTotal(data.total[idx]), numeric: true }]
            : []),
        ],
        color:
          data.graphType === "heatmap"
            ? undefined
            : color(
                uniq.findIndex((o) => sameRow(row, o)),
                false,
                theme,
              ),
      });
    });
    return { columns: [...names, ...stats], numeric: stats.length, rows };
  }, [data, displayedAxis, config, theme]);

  if (!table) return null;
  const hasColors = table.rows.some((row) => row.color);
  const firstNumeric = table.columns.length - table.numeric;
  const canHighlight =
    data && data.graphType !== "sankey" && data.graphType !== "heatmap";
  return (
    <div className="flex flex-col gap-3">
      {axes.length > 1 && (
        <Tabs
          value={displayedAxis.toString()}
          onValueChange={(value) => setSelectedAxis(Number(value))}
        >
          <TabsList>
            {axes.map((axis) => (
              <TabsTrigger key={axis.id} value={axis.id.toString()}>
                {axis.name}
              </TabsTrigger>
            ))}
          </TabsList>
        </Tabs>
      )}
      <Table>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            {hasColors && <TableHead className="w-8" />}
            {table.columns.map((column, idx) => (
              <TableHead
                key={column}
                className={cn(
                  "text-2xs font-semibold uppercase tracking-label text-muted-foreground",
                  idx >= firstNumeric && "text-right",
                )}
              >
                {column}
              </TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody>
          {table.rows.map((row) => (
            <TableRow
              key={row.index}
              onPointerEnter={() => canHighlight && onHighlight(row.index)}
              onPointerLeave={() => onHighlight(null)}
            >
              {hasColors && (
                <TableCell>
                  {row.color && (
                    <span
                      className="block size-3.5 rounded"
                      style={{
                        backgroundColor: row.color,
                        printColorAdjust: "exact",
                      }}
                    />
                  )}
                </TableCell>
              )}
              {row.cells.map((cell, idx) => (
                <TableCell
                  key={idx}
                  className={cn(
                    cell.numeric && "text-right font-mono text-xs tabular-nums",
                  )}
                >
                  {cell.value}
                </TableCell>
              ))}
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
