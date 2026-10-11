// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";
import { endDate, longDate } from "./dates";

// Dates are built from local parts: the result does not depend on the time
// zone of the machine.
const at = (day: number, hours: number, minutes: number) =>
  new Date(2026, 9, day, hours, minutes).toISOString();

describe("longDate", () => {
  it("uses a 24-hour clock", () => {
    expect(longDate(at(10, 0, 59))).toBe("October 10, 2026 00:59");
    expect(longDate(at(10, 18, 5))).toBe("October 10, 2026 18:05");
  });
});

describe("endDate", () => {
  it("keeps the time only on the same day", () => {
    expect(endDate(at(10, 0, 59), at(10, 18, 5))).toBe("18:05");
  });

  it("keeps the date on another day", () => {
    expect(endDate(at(10, 0, 59), at(11, 6, 30))).toBe("October 11, 2026 06:30");
  });
});
