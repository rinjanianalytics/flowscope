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

export const unitLabels = {
  l3bps: "L3ᵇ⁄ₛ",
  l2bps: "L2ᵇ⁄ₛ",
  "inl2%": "→L2%",
  "outl2%": "L2%→",
  pps: "ᵖ⁄ₛ",
  fps: "ᶠ⁄ₛ",
} as const;
