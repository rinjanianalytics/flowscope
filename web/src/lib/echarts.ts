// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

// The parts of ECharts the console uses.

import {
  use as register,
  graphic,
  init,
  type ComposeOption,
  type ECharts,
} from "echarts/core";
import { CanvasRenderer } from "echarts/renderers";
import {
  LineChart,
  HeatmapChart,
  PieChart,
  SankeyChart,
  type LineSeriesOption,
  type HeatmapSeriesOption,
  type PieSeriesOption,
  type SankeySeriesOption,
} from "echarts/charts";
import {
  TooltipComponent,
  GridComponent,
  BrushComponent,
  ToolboxComponent,
  DatasetComponent,
  TitleComponent,
  VisualMapComponent,
  LegendComponent,
  type TooltipComponentOption,
  type GridComponentOption,
  type BrushComponentOption,
  type ToolboxComponentOption,
  type DatasetComponentOption,
  type TitleComponentOption,
  type VisualMapComponentOption,
  type LegendComponentOption,
} from "echarts/components";

register([
  CanvasRenderer,
  LineChart,
  HeatmapChart,
  PieChart,
  SankeyChart,
  TooltipComponent,
  GridComponent,
  ToolboxComponent,
  BrushComponent,
  DatasetComponent,
  TitleComponent,
  VisualMapComponent,
  LegendComponent,
]);

export type ECOption = ComposeOption<
  | LineSeriesOption
  | HeatmapSeriesOption
  | PieSeriesOption
  | SankeySeriesOption
  | TooltipComponentOption
  | GridComponentOption
  | BrushComponentOption
  | ToolboxComponentOption
  | DatasetComponentOption
  | TitleComponentOption
  | VisualMapComponentOption
  | LegendComponentOption
>;

export { graphic, init };
export type { ECharts, LineSeriesOption, SankeySeriesOption };
