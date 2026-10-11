// SPDX-FileCopyrightText: 2025 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";
import {
  formatXps,
  formatXpsLabel,
  formatSpan,
  compareFields,
  groupFlow,
  reverseDimension,
} from "./format";

describe("formatXps", () => {
  it("formats small values without suffix", () => {
    expect(formatXps(0)).toBe("0.00");
    expect(formatXps(1)).toBe("1.00");
    expect(formatXps(999)).toBe("999.00");
  });

  it("preserves negative sign", () => {
    expect(formatXps(-1)).toBe("-1.00");
    expect(formatXps(-1000)).toBe("-1.00K");
    expect(formatXps(-1500)).toBe("-1.50K");
  });

  it("formats values with K suffix", () => {
    expect(formatXps(1000)).toBe("1.00K");
    expect(formatXps(1500)).toBe("1.50K");
    expect(formatXps(999999)).toBe("1000.00K");
  });

  it("formats values with M suffix", () => {
    expect(formatXps(1000000)).toBe("1.00M");
    expect(formatXps(1500000)).toBe("1.50M");
    expect(formatXps(999999999)).toBe("1000.00M");
  });

  it("formats values with G suffix", () => {
    expect(formatXps(1000000000)).toBe("1.00G");
    expect(formatXps(1500000000)).toBe("1.50G");
    expect(formatXps(999999999999)).toBe("1000.00G");
  });

  it("formats values with T suffix", () => {
    expect(formatXps(1000000000000)).toBe("1.00T");
    expect(formatXps(1500000000000)).toBe("1.50T");
    expect(formatXps(999999999999999)).toBe("1000.00T");
  });

  it("formats values with P suffix", () => {
    expect(formatXps(1000000000000000)).toBe("1.00P");
    expect(formatXps(1500000000000000)).toBe("1.50P");
  });

  it("caps at P suffix for very large values", () => {
    expect(formatXps(1000000000000000000)).toBe("1000.00P");
    expect(formatXps(9999999999999999999)).toBe("10000.00P");
  });

  it("rounds to 2 decimal places", () => {
    expect(formatXps(1234)).toBe("1.23K");
    expect(formatXps(1236)).toBe("1.24K");
    expect(formatXps(1234567)).toBe("1.23M");
  });
});

describe("formatXpsLabel", () => {
  it("adds the unit and drops the zeros at the end", () => {
    expect(formatXpsLabel(0, "bps")).toBe("0 bps");
    expect(formatXpsLabel(150_000_000_000, "bps")).toBe("150 Gbps");
    expect(formatXpsLabel(1_500_000, "pps")).toBe("1.5 Mpps");
    expect(formatXpsLabel(62_250, "fps")).toBe("62.25 Kfps");
  });

  it("preserves negative sign", () => {
    expect(formatXpsLabel(-2_000_000_000, "bps")).toBe("-2 Gbps");
  });
});

describe("formatSpan", () => {
  it("uses the largest unit which reads well", () => {
    expect(formatSpan(30 * 60_000)).toBe("30 minutes");
    expect(formatSpan(3_600_000)).toBe("1 hour");
    expect(formatSpan(24 * 3_600_000)).toBe("24 hours");
    expect(formatSpan(7 * 24 * 3_600_000)).toBe("7 days");
  });

  it("rounds a duration which is a few minutes short", () => {
    expect(formatSpan(24 * 3_600_000 - 7 * 60_000)).toBe("24 hours");
  });
});

describe("groupFlow", () => {
  const flow = {
    SrcAddr: "192.0.2.152",
    SrcCountry: "",
    DstAS: 20940,
    DstASPath: [174, 20940],
    DstLargeCommunities: [],
    Dst3rdAS: 0,
    InIfName: "Gi0/0/0/20",
    OutIfProvider: "",
    OutIfName: "Gi0/0/0/11",
    ExporterName: "dc5-edge2.example.com",
    Bytes: 120,
    TimeReceived: "2026-10-11T01:23:53Z",
    Proto: 6,
  };

  it("puts the fields in sections, in order", () => {
    expect(groupFlow(flow).sections).toEqual([
      {
        title: "Flow",
        fields: [
          ["TimeReceived", "2026-10-11T01:23:53Z"],
          ["Bytes", "120"],
          ["Proto", "6"],
        ],
      },
      { title: "Exporter", fields: [["ExporterName", "dc5-edge2.example.com"]] },
      { title: "Source", fields: [["SrcAddr", "192.0.2.152"]] },
      {
        title: "Destination",
        fields: [
          ["Dst3rdAS", "0"],
          ["DstAS", "20940"],
          ["DstASPath", "174 20940"],
        ],
      },
      {
        title: "Interfaces",
        fields: [
          ["InIfName", "Gi0/0/0/20"],
          ["OutIfName", "Gi0/0/0/11"],
        ],
      },
    ]);
  });

  it("returns the fields without a value apart", () => {
    expect(groupFlow(flow).empty).toEqual([
      "SrcCountry",
      "DstLargeCommunities",
      "OutIfProvider",
    ]);
  });

  it("drops a section without a field", () => {
    expect(groupFlow({ Bytes: 1 }).sections.map((s) => s.title)).toEqual([
      "Flow",
    ]);
  });
});

describe("compareFields", () => {
  it("compares fields by prefix priority", () => {
    expect(compareFields("TimeReceived", "Bytes")).toBeLessThan(0);
    expect(compareFields("Bytes", "Packets")).toBeLessThan(0);
    expect(compareFields("ExporterName", "SamplingRate")).toBeLessThan(0);
    expect(compareFields("SamplingRate", "SrcCountry")).toBeLessThan(0);
    expect(compareFields("SrcCountry", "InIfName")).toBeLessThan(0);
    expect(compareFields("InIfBoundary", "DstAS")).toBeLessThan(0);
    expect(compareFields("DstNetNask", "OutIfDescription")).toBeLessThan(0);
    expect(compareFields("OutIfName", "Proto")).toBeLessThan(0);
    expect(compareFields("OutIfName", "EType")).toBeLessThan(0);
  });

  it("compares fields with same prefix alphabetically", () => {
    expect(compareFields("DstAddr", "DstAS")).toBeLessThan(0);
    expect(compareFields("SrcAddr", "SrcAS")).toBeLessThan(0);
  });

  it("handles unknown prefixes with fallback priority", () => {
    expect(compareFields("UnkField1", "UnkField2")).toBeLessThan(0);
    expect(compareFields("UnkField2", "UnkField1")).toBeGreaterThan(0);
    expect(compareFields("UnkField", "TimeReceived")).toBeGreaterThan(0);
    expect(compareFields("TimeReceived", "UnkField")).toBeLessThan(0);
  });

  it("handles equal fields", () => {
    expect(compareFields("TimeReceived", "TimeReceived")).toBe(0);
    expect(compareFields("SrcAddr", "SrcAddr")).toBe(0);
  });
});

describe("reverseDimension", () => {
  const known = [
    "SrcAS",
    "DstAS",
    "SrcAddr",
    "DstAddr",
    "InIfName",
    "OutIfName",
    "SrcNetName",
    "ExporterName",
    "Proto",
  ];

  it("swaps Src and Dst", () => {
    expect(reverseDimension("SrcAS", known)).toBe("DstAS");
    expect(reverseDimension("DstAS", known)).toBe("SrcAS");
    expect(reverseDimension("SrcAddr", known)).toBe("DstAddr");
  });

  it("swaps In and Out", () => {
    expect(reverseDimension("InIfName", known)).toBe("OutIfName");
    expect(reverseDimension("OutIfName", known)).toBe("InIfName");
  });

  it("keeps dimensions without a direction", () => {
    expect(reverseDimension("ExporterName", known)).toBe("ExporterName");
    expect(reverseDimension("Proto", known)).toBe("Proto");
  });

  it("keeps dimensions whose opposite is unknown", () => {
    expect(reverseDimension("SrcNetName", known)).toBe("SrcNetName");
    expect(reverseDimension("SrcAS", [])).toBe("SrcAS");
  });
});
