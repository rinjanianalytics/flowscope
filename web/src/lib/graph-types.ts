// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

export const graphTypes = {
  stacked: "Stacked areas",
  stacked100: "100% stacked",
  lines: "Lines",
  grid: "Grid",
  sankey: "Sankey",
  heatmap: "Heatmap",
} as const;
export type GraphType = keyof typeof graphTypes;

export const bidirectionalGraphTypes: GraphType[] = [
  "stacked",
  "stacked100",
  "lines",
  "grid",
  "sankey",
];
export const previousPeriodGraphTypes: GraphType[] = ["stacked"];

/** Units, short, for where the room is scarce. */
export const unitLabels = {
  l3bps: "L3 bits/s",
  l2bps: "L2 bits/s",
  "inl2%": "% of input interface",
  "outl2%": "% of output interface",
  pps: "Packets/s",
  fps: "Flows/s",
} as const;

/** Units, in full. */
export const unitNames = {
  l3bps: "Layer 3 bits per second",
  l2bps: "Layer 2 bits per second",
  "inl2%": "Share of the input interface",
  "outl2%": "Share of the output interface",
  pps: "Packets per second",
  fps: "Flows per second",
} as const;
