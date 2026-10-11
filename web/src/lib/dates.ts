// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

// Dates written by humans ("6 hours ago").

// @ts-expect-error The types shipped with sugar-date are not a module.
import Sugar from "sugar-date";

type Chainable = {
  raw: string;
  format(format?: string): Chainable;
  long(): Chainable;
  toDateString(): Chainable;
};
const SugarDate = (Sugar as { Date: unknown }).Date as {
  (date: Date | string): Chainable;
  create(input: string): Date;
};

/** Parse a date written by a human. The result may be an invalid date. */
export const parseDate = (input: string): Date => SugarDate.create(input);

export const isValidDate = (date: Date) => !isNaN(date.valueOf());

export const formatDate = (date: Date) =>
  SugarDate(date).format("{yyyy}-{MM}-{dd} {HH}:{mm}:{ss}").raw;

// A 24-hour clock, like the axes of the graphs.
const long = "{Month} {d}, {yyyy} {HH}:{mm}";

/** Long form of a date, for display. */
export const longDate = (iso: string) => SugarDate(iso).format(long).raw;

/** Time only when both dates are on the same day. */
export const endDate = (startIso: string, endIso: string) =>
  SugarDate(endIso).format(
    SugarDate(startIso).toDateString().raw ===
      SugarDate(endIso).toDateString().raw
      ? "{HH}:{mm}"
      : long,
  ).raw;
