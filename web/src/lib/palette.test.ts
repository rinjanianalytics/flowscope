// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";
import { dataColor } from "./palette";

describe("dataColor", () => {
  it.each(["light", "dark"] as const)(
    "gives a color of its own to each of 30 series (%s)",
    (theme) => {
      const colors = Array.from({ length: 30 }, (_, index) =>
        dataColor(index, false, theme),
      );
      expect(new Set(colors).size).toBe(30);
      for (const color of colors) expect(color).toMatch(/^#[0-9a-f]{6}$/);
    },
  );

  it("starts again after the last color", () => {
    expect(dataColor(30)).toBe(dataColor(0));
  });

  it("gives another color for the alternate", () => {
    expect(dataColor(0, true)).not.toBe(dataColor(0));
  });
});
