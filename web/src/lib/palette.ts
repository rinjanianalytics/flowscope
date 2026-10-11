// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

// Colors of the data series, built from the chart colors of the brand kit.

type Theme = "light" | "dark";

// Ten colors which differ in hue and in lightness, so two neighbors in a
// stacked graph stay apart. The first and the third are chart-1 and chart-3
// of the brand kit.
const hues: Record<Theme, string[]> = {
  light: [
    "#118a78",
    "#e9a23b",
    "#4c72d9",
    "#e58fc4",
    "#7a4a12",
    "#6fcdbd",
    "#7c3f8e",
    "#b9c96a",
    "#c2413b",
    "#9db4f0",
  ],
  dark: [
    "#1fa08b",
    "#e9a23b",
    "#5c86e6",
    "#e58fc4",
    "#a8762f",
    "#7fdccb",
    "#a36bb8",
    "#c6d67a",
    "#e0655f",
    "#b3c4f5",
  ],
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

// After the last color, the hues repeat with another shade: lighter on a
// light background, darker on a dark one.
const palettes: Record<Theme, string[]> = {
  light: [0, 0.35, 0.6].flatMap((amount) =>
    hues.light.map((color) => mix(color, "#ffffff", amount)),
  ),
  dark: [0, 0.3, 0.52].flatMap((amount) =>
    hues.dark.map((color) => mix(color, "#0f1215", amount)),
  ),
};

export function dataColor(
  index: number,
  alternate = false,
  theme: Theme = "light",
) {
  const palette = palettes[theme];
  // The index may be negative: the grid asks for the color of a row it does
  // not draw.
  const computed =
    palette[((index % palette.length) + palette.length) % palette.length];
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
