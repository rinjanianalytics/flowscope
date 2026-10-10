// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

// Colors of the data series, built from the chart colors of the brand kit.

type Theme = "light" | "dark";

// chart-1 to chart-4, for each theme.
const hues: Record<Theme, string[]> = {
  light: ["#118a78", "#c97c14", "#4c72d9", "#b5508f"],
  dark: ["#1fa08b", "#c97c14", "#5c86e6", "#c860a4"],
};
// chart-5 and neighbors, for what is not a named series ("Other").
const greys: Record<Theme, string[]> = {
  light: ["#9aa3ae", "#b3bac3", "#838d99", "#c9ced5", "#6f7985"],
  dark: ["#6b7480", "#7d8691", "#59626d", "#8f98a3", "#4a525c"],
};

const channels = (color: string) =>
  [1, 3, 5].map((i) => parseInt(color.slice(i, i + 2), 16));
const hex = (values: number[]) =>
  "#" +
  values
    .map((v) =>
      Math.round(Math.min(255, Math.max(0, v)))
        .toString(16)
        .padStart(2, "0"),
    )
    .join("");

/** Mix a color with another one. */
const mix = (color: string, target: string, amount: number) => {
  const to = channels(target);
  return hex(channels(color).map((v, i) => v + (to[i] - v) * amount));
};

const lightenColor = (color: string, amount: number) =>
  hex(channels(color).map((v) => v + amount));

// Each step repeats the hues with another shade: lighter on a light
// background, darker on a dark one.
const palettes: Record<Theme, string[]> = {
  light: [0, 0.22, 0.42, 0.58, 0.72, 0.84].flatMap((amount) =>
    hues.light.map((color) => mix(color, "#ffffff", amount)),
  ),
  dark: [0, 0.16, 0.32, 0.46, 0.58, 0.7].flatMap((amount) =>
    hues.dark.map((color) => mix(color, "#0f1215", amount)),
  ),
};

export function dataColor(
  index: number,
  alternate = false,
  theme: Theme = "light",
) {
  const palette = palettes[theme];
  // Neighbors get a different shade as well as a different hue.
  const correctedIndex = index % 2 === 0 ? index : index + hues.light.length;
  const computed = palette[correctedIndex % palette.length];
  if (!alternate) {
    return computed;
  }
  return lightenColor(computed, 20);
}

export function dataColorGrey(
  index: number,
  alternate = false,
  theme: Theme = "light",
) {
  const palette = greys[theme];
  const computed = palette[index % palette.length];
  if (!alternate) {
    return computed;
  }
  return lightenColor(computed, 10);
}

/** Colors of the axes and of the grid of a chart. */
export const chartChrome = (theme: Theme) =>
  theme === "light"
    ? { grid: "#eceef1", axis: "#c9ced5", text: "#5a6270", ink: "#16181c" }
    : { grid: "#262b31", axis: "#3a4148", text: "#a3adb6", ink: "#e8ecef" };
