// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

const suffixes = ["", "K", "M", "G", "T", "P"];

/** A value as a multiple of a power of 1000, with the suffix of this power. */
export function scaleXps(value: number) {
  let scaled = Math.abs(value);
  let idx = 0;
  while (scaled >= 1000 && idx < suffixes.length - 1) {
    scaled /= 1000;
    idx++;
  }
  return { value: value < 0 ? -scaled : scaled, suffix: suffixes[idx] };
}

export function formatXps(value: number) {
  const { value: scaled, suffix } = scaleXps(value);
  return `${scaled.toFixed(2)}${suffix}`;
}

/**
 * A value with its unit and without the zeros at the end ("150 Gbps"), for
 * the labels of an axis.
 */
export function formatXpsLabel(value: number, unit: string) {
  const { value: scaled, suffix } = scaleXps(value);
  return `${parseFloat(scaled.toFixed(2))} ${suffix}${unit}`;
}

/** A duration in words ("24 hours", "7 days"), from milliseconds. */
export function formatSpan(duration: number) {
  const plural = (count: number, unit: string) =>
    `${count} ${unit}${count === 1 ? "" : "s"}`;
  const minutes = Math.round(duration / 60_000);
  if (minutes < 60) return plural(Math.max(1, minutes), "minute");
  const hours = Math.round(minutes / 60);
  return hours < 48
    ? plural(hours, "hour")
    : plural(Math.round(hours / 24), "day");
}

// Order function for field names
export function compareFields(f1: string, f2: string) {
  const metric: { [prefix: string]: number } = {
    Tim: 1,
    Byt: 2,
    Pac: 3,
    Exp: 7,
    Sam: 8,
    Src: 10,
    Dst: 12,
    InI: 11,
    Out: 13,
  };
  const m1 = metric[f1.substring(0, 3)] || 100;
  const m2 = metric[f2.substring(0, 3)] || 100;
  const cmp = m1 - m2;
  if (cmp !== 0) {
    return cmp;
  }
  return f1.localeCompare(f2);
}

/** The value of a field of a flow, as text. */
export const flowValue = (value: unknown) =>
  Array.isArray(value) ? value.join(" ") : String(value ?? "");

const flowGroups: { title: string; prefixes: string[] }[] = [
  { title: "Exporter", prefixes: ["Exporter"] },
  { title: "Source", prefixes: ["Src"] },
  { title: "Destination", prefixes: ["Dst"] },
  { title: "Interfaces", prefixes: ["InIf", "OutIf"] },
];

/**
 * The fields of a flow in sections, in order. The fields without a value are
 * returned apart, by name only: there are many and they say little. A zero
 * is a value.
 */
export function groupFlow(flow: Record<string, unknown>) {
  const sections = ["Flow", ...flowGroups.map(({ title }) => title)].map(
    (title) => ({ title, fields: [] as [string, string][] }),
  );
  const empty: string[] = [];
  for (const field of Object.keys(flow).sort(compareFields)) {
    const value = flowValue(flow[field]);
    if (value === "") {
      empty.push(field);
      continue;
    }
    const group = flowGroups.findIndex(({ prefixes }) =>
      prefixes.some((prefix) => field.startsWith(prefix)),
    );
    sections[group + 1].fields.push([field, value]);
  }
  return {
    sections: sections.filter(({ fields }) => fields.length > 0),
    empty,
  };
}

// Reverse the direction of a dimension name, the same way the backend does
// for the reverse axis of a bidirectional query. The candidate name is only
// used when it is part of the known dimensions, otherwise the name is kept
// unchanged.
export function reverseDimension(name: string, known: readonly string[]) {
  const prefixes = [
    ["Src", "Dst"],
    ["Dst", "Src"],
    ["In", "Out"],
    ["Out", "In"],
  ];
  for (const [from, to] of prefixes) {
    if (name.startsWith(from)) {
      const candidate = `${to}${name.slice(from.length)}`;
      return known.includes(candidate) ? candidate : name;
    }
  }
  return name;
}

/** Name of a row of dimension values. */
export const rowName = (row: string[]) => row.join(" — ") || "Total";

/** Tell if two rows have the same dimension values. */
export const sameRow = (a: string[], b: string[]) =>
  a.length === b.length && a.every((v, i) => v === b[i]);

/** Rows without duplicates, in order. */
export const uniqueRows = (rows: string[][]) =>
  rows.filter((row, idx) => rows.findIndex((o) => sameRow(o, row)) === idx);
