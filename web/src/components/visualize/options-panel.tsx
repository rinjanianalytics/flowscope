// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeftRight,
  ChartArea,
  ChartLine,
  ChartNoAxesGantt,
  ChevronLeft,
  ChevronRight,
  Grid2x2,
  Grid3x3,
  Percent,
  SlidersHorizontal,
  ZoomOut,
  type LucideIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import {
  Combobox,
  Field,
  SectionTitle,
  TextField,
  iconButton,
} from "@/components/visualize/field";
import { DimensionsInput } from "@/components/visualize/dimensions-input";
import { FilterInput } from "@/components/visualize/filter-input";
import { useServerConfig } from "@/components/providers";
import { apiFetch, type Units } from "@/lib/api";
import { formatDate, isValidDate, parseDate } from "@/lib/dates";
import {
  bidirectionalGraphTypes,
  graphTypes,
  previousPeriodGraphTypes,
  unitNames,
  type GraphType,
} from "@/lib/graph-types";
import { isEqual, type VisualizeState } from "@/lib/state";
import { cn } from "@/lib/utils";
import { useMediaQuery } from "@/hooks/use-media-query";
import { useScrollFade } from "@/hooks/use-scroll-fade";

const graphIcons: Record<GraphType, LucideIcon> = {
  stacked: ChartArea,
  stacked100: Percent,
  lines: ChartLine,
  grid: Grid2x2,
  sankey: ChartNoAxesGantt,
  heatmap: Grid3x3,
};

const units = Object.entries(unitNames).map(([value, label]) => ({
  value: value as Units,
  label,
}));

const limitTypes = [
  { value: "avg", label: "Avg" },
  { value: "max", label: "Max" },
  { value: "last", label: "Last" },
];

const presets: { name: string; start?: string; end?: string }[] = [
  { name: "Custom" },
  { name: "Last hour", start: "1 hour ago", end: "now" },
  { name: "Last 6 hours", start: "6 hours ago", end: "now" },
  { name: "Last 12 hours", start: "12 hours ago", end: "now" },
  { name: "Last 24 hours", start: "24 hours ago", end: "now" },
  { name: "Last evening", start: "yesterday at 7pm", end: "today at 1am" },
  { name: "Last 2 days", start: "2 days ago", end: "now" },
  { name: "Last 7 days", start: "7 days ago", end: "now" },
  { name: "Last 30 days", start: "30 days ago", end: "now" },
  { name: "Last 3 months", start: "3 months ago", end: "now" },
  { name: "Last 6 months", start: "6 months ago", end: "now" },
  { name: "Last year", start: "1 year ago", end: "now" },
  { name: "Last 2 years", start: "2 years ago", end: "now" },
  { name: "Last 5 years", start: "5 years ago", end: "now" },
  { name: "Today", start: "today", end: "end of today" },
  { name: "Yesterday", start: "yesterday", end: "end of yesterday" },
  {
    name: "Day before yesterday",
    start: "day before yesterday",
    end: "yesterday",
  },
  {
    name: "This week",
    start: "the beginning of this week",
    end: "the end of this week",
  },
  {
    name: "This month",
    start: "the beginning of this month",
    end: "the end of this month",
  },
  {
    name: "This year",
    start: "the beginning of this year",
    end: "the end of this year",
  },
  { name: "This day last week", start: "0am 1 week ago", end: "0am 6 days ago" },
  {
    name: "Previous week",
    start: "the beginning of last week",
    end: "the end of last week",
  },
  {
    name: "Previous month",
    start: "the beginning of last month",
    end: "the end of last month",
  },
  {
    name: "Previous year",
    start: "the beginning of last year",
    end: "the end of last year",
  },
];

// Querying again only brings new data when the range follows the clock, so
// this is limited to a range ending with "now".
const refreshIntervals = [
  { value: 0, label: "off" },
  { value: 5_000, label: "5s" },
  { value: 10_000, label: "10s" },
  { value: 30_000, label: "30s" },
  { value: 60_000, label: "1m" },
  { value: 300_000, label: "5m" },
];

/** What the user is editing. Numbers are kept as typed. */
type Fields = {
  graphType: GraphType;
  humanStart: string;
  humanEnd: string;
  dimensions: string[];
  limit: string;
  limitType: string;
  truncate4: string;
  truncate6: string;
  filter: string;
  units: Units;
  bidirectional: boolean;
  previousPeriod: boolean;
};

/** A state without the resolved dates. */
type Options = Omit<VisualizeState, "start" | "end">;

const toOptions = (fields: Fields): Options => ({
  graphType: fields.graphType,
  humanStart: fields.humanStart,
  humanEnd: fields.humanEnd,
  dimensions: fields.dimensions,
  limit: parseInt(fields.limit),
  limitType: fields.limitType,
  "truncate-v4": parseInt(fields.truncate4),
  "truncate-v6": parseInt(fields.truncate6),
  filter: fields.filter,
  units: fields.units,
  bidirectional:
    bidirectionalGraphTypes.includes(fields.graphType) && fields.bidirectional,
  previousPeriod:
    previousPeriodGraphTypes.includes(fields.graphType) &&
    fields.previousPeriod,
});

const withoutDates = (state: VisualizeState): Options => {
  const { start: _start, end: _end, ...options } = state;
  return options;
};

export function OptionsPanel({
  value,
  loading,
  onChange,
  onCancel,
}: {
  value: VisualizeState | null;
  loading: boolean;
  /** `auto` is set when the change comes from the auto refresh. */
  onChange: (value: VisualizeState, auto?: boolean) => void;
  onCancel: () => void;
}) {
  const config = useServerConfig();
  const defaults = config?.defaultVisualizeOptions;
  // Next to the graph when there is room for both. Otherwise, the graph
  // comes first and the options open in a sheet.
  const wide = useMediaQuery("(min-width: 64rem)", true);
  const [open, setOpen] = useState(true);
  const [sheetOpen, setSheetOpen] = useState(false);
  const [fields, setFields] = useState<Fields | null>(null);
  const [filterInvalid, setFilterInvalid] = useState(false);
  const fadeRef = useScrollFade<HTMLDivElement>();
  const [refreshInterval, setAutoRefresh] = useState(0);
  const set = (update: Partial<Fields>) =>
    setFields((current) => (current ? { ...current, ...update } : current));

  // Errors
  const start = useMemo(
    () => parseDate(fields?.humanStart ?? ""),
    [fields?.humanStart],
  );
  const end = useMemo(
    () => parseDate(fields?.humanEnd ?? ""),
    [fields?.humanEnd],
  );
  const startError = isValidDate(start) ? "" : "Invalid date";
  const endError = !isValidDate(end)
    ? "Invalid date"
    : isValidDate(start) && start > end
      ? "End date should be before start date"
      : "";
  const timeErrors = !!(startError || endError);
  const minDimensions = fields?.graphType === "sankey" ? 2 : 0;
  const dimensionsError =
    (fields?.dimensions.length ?? 0) < minDimensions
      ? "At least two dimensions are required"
      : "";
  const limitError = (() => {
    const val = parseInt(fields?.limit ?? "");
    if (isNaN(val)) return "Not a number";
    if (val < 1) return "Should be ≥ 1";
    const upper = config?.dimensionsLimit ?? 50;
    if (val > upper) return `Should be ≤ ${upper}`;
    return "";
  })();
  const truncateError = (text: string | undefined, max: number) => {
    const val = parseInt(text ?? "");
    return isNaN(val) || val <= 0 || val > max ? `0 < x ≤ ${max}` : "";
  };
  const truncate4Error = truncateError(fields?.truncate4, 32);
  const truncate6Error = truncateError(fields?.truncate6, 128);
  const hasErrors =
    timeErrors ||
    !!dimensionsError ||
    !!limitError ||
    !!truncate4Error ||
    !!truncate6Error ||
    filterInvalid;
  const canAggregate = (fields?.dimensions ?? []).some((name) =>
    (config?.truncatable ?? []).includes(name),
  );

  // Submit what is displayed. `force` submits even when a query is running
  // (otherwise the button cancels it).
  const latest = useRef({ fields, hasErrors, loading });
  useEffect(() => {
    latest.current = { fields, hasErrors, loading };
  });
  const submit = (force?: boolean, auto?: boolean, override?: Partial<Fields>) => {
    const current = latest.current;
    if (!force && current.loading) {
      onCancel();
      return;
    }
    if (!current.fields) return;
    const next = { ...current.fields, ...override };
    // An override comes from a button which is disabled on errors.
    if (!override && current.hasErrors) return;
    onChange(
      {
        ...toOptions(next),
        start: parseDate(next.humanStart).toISOString(),
        end: parseDate(next.humanEnd).toISOString(),
      },
      auto,
    );
  };

  // Follow the value, or the defaults when there is none.
  useEffect(() => {
    if (!defaults) return;
    const current: Options = value
      ? withoutDates(value)
      : {
          graphType: defaults.graphType,
          humanStart: defaults.start,
          humanEnd: defaults.end,
          dimensions: defaults.dimensions,
          limit: defaults.limit,
          limitType: defaults.limitType,
          "truncate-v4": 32,
          "truncate-v6": 128,
          filter: defaults.filter,
          units: "l3bps",
          bidirectional: defaults.bidirectional,
          previousPeriod: defaults.previousPeriod,
        };
    const next: Fields = {
      graphType: current.graphType in graphTypes ? current.graphType : "stacked",
      humanStart: current.humanStart,
      humanEnd: current.humanEnd,
      dimensions: [...current.dimensions],
      limit: current.limit.toString(),
      limitType: limitTypes.some((l) => l.value === current.limitType)
        ? current.limitType
        : "avg",
      truncate4: (current["truncate-v4"] || 32).toString(),
      truncate6: (current["truncate-v6"] || 128).toString(),
      filter: current.filter,
      units: current.units,
      bidirectional: current.bidirectional,
      previousPeriod: current.previousPeriod,
    };
    // The fields follow the value, which also changes from outside (URL,
    // selection in the graph).
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setFields(next);
    // Query when there is no result for these options yet.
    if (
      value === null ||
      !value.start ||
      !value.end ||
      !isEqual(withoutDates(value), toOptions(next))
    ) {
      setOpen(true);
      const startDate = parseDate(next.humanStart);
      const endDate = parseDate(next.humanEnd);
      if (isValidDate(startDate) && isValidDate(endDate)) {
        onChange({
          ...toOptions(next),
          start: startDate.toISOString(),
          end: endDate.toISOString(),
        });
      }
    }
    // onChange is stable enough: the page keeps it in a callback.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value, defaults]);

  // Auto refresh
  const liveRange = fields?.humanEnd.trim().toLowerCase() === "now";
  const autoRefresh = liveRange ? refreshInterval : 0;
  useEffect(() => {
    if (!autoRefresh) return;
    const timer = setInterval(() => {
      // Skip the tick while a query runs: a query slower than the interval
      // would keep cancelling and restarting itself.
      if (!latest.current.loading) submit(true, true);
    }, autoRefresh);
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoRefresh]);

  if (!fields) {
    return <aside className="shrink-0 lg:w-80 print:hidden" />;
  }

  // What the button of the form does. The sheet goes away to show the result.
  const apply = () => {
    submit();
    if (!hasErrors) setSheetOpen(false);
  };

  // Time range helpers
  const shiftTimeRange = (direction: number) => {
    if (timeErrors) return;
    const offset = direction * (end.valueOf() - start.valueOf());
    const update = {
      humanStart: formatDate(new Date(start.valueOf() + offset)),
      humanEnd: formatDate(new Date(end.valueOf() + offset)),
    };
    set(update);
    submit(true, false, update);
  };
  // Double the duration of the time range, keeping the same center.
  const zoomOut = () => {
    if (timeErrors) return;
    const duration = end.valueOf() - start.valueOf();
    const update = liveRange
      ? { humanStart: formatDate(new Date(start.valueOf() - duration)) }
      : {
          humanStart: formatDate(new Date(start.valueOf() - duration / 2)),
          humanEnd: formatDate(new Date(end.valueOf() + duration / 2)),
        };
    set(update);
    submit(true, false, update);
  };
  const selectedPreset =
    presets.find(
      (p) => p.start === fields.humanStart && p.end === fields.humanEnd,
    ) ?? presets[0];

  // Reverse the direction, with the help of the server.
  const reverse = (input: { dimensions?: string[]; filter?: string }) =>
    apiFetch<{ dimensions: string[]; filter: string }>("graph/reverse", {
      json: input,
    }).catch(() => null);
  const reverseDimensions = async () => {
    const data = await reverse({ dimensions: fields.dimensions });
    if (data) set({ dimensions: data.dimensions });
  };
  const reverseFilter = async () => {
    const data = await reverse({ filter: fields.filter });
    if (data) set({ filter: data.filter });
  };

  const applyLabel =
    value && isEqual(toOptions(fields), withoutDates(value))
      ? "Refresh"
      : "Apply";
  const shiftButton = (direction: number, title: string) => (
    <button
      type="button"
      title={title}
      disabled={timeErrors}
      className={iconButton}
      onClick={() => shiftTimeRange(direction)}
    >
      {direction < 0 ? (
        <ChevronLeft className="size-4" />
      ) : (
        <ChevronRight className="size-4" />
      )}
    </button>
  );

  const form = (
    <form
      className="flex min-h-0 flex-1 flex-col"
      autoComplete="off"
      spellCheck={false}
      onSubmit={(event) => {
        event.preventDefault();
        apply();
      }}
      onKeyDownCapture={(event) => {
        if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
          // The filter editor has its own handling.
          if ((event.target as Element).closest(".cm-editor")) return;
          event.preventDefault();
          apply();
        }
      }}
    >
      <div
        ref={fadeRef}
        className="no-scrollbar scroll-fade flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-4 pb-4 pt-2"
      >
            <SectionTitle>Graph</SectionTitle>
            <Field label="Unit">
              <Select
                value={fields.units}
                onValueChange={(unit) => set({ units: unit as Units })}
                items={units}
              >
                <SelectTrigger aria-label="Unit" className="w-full bg-card">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {units.map((unit) => (
                    <SelectItem key={unit.value} value={unit.value}>
                      {unit.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
            <Field label="Graph type">
              <Select
                value={fields.graphType}
                onValueChange={(type) => set({ graphType: type as GraphType })}
                items={Object.entries(graphTypes).map(([value, label]) => ({
                  value,
                  label,
                }))}
              >
                <SelectTrigger aria-label="Graph type" className="w-full bg-card">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {(Object.keys(graphTypes) as GraphType[]).map((type) => {
                    const Icon = graphIcons[type];
                    return (
                      <SelectItem key={type} value={type}>
                        <Icon className="size-4 text-muted-foreground" />
                        {graphTypes[type]}
                      </SelectItem>
                    );
                  })}
                </SelectContent>
              </Select>
            </Field>
            <div className="flex flex-wrap gap-x-4 gap-y-1">
              {bidirectionalGraphTypes.includes(fields.graphType) && (
                <label className="flex cursor-pointer items-center gap-2 text-sm">
                  <Checkbox
                    checked={fields.bidirectional}
                    onCheckedChange={(checked) =>
                      set({ bidirectional: !!checked })
                    }
                  />
                  Bidirectional
                </label>
              )}
              {previousPeriodGraphTypes.includes(fields.graphType) && (
                <label className="flex cursor-pointer items-center gap-2 text-sm">
                  <Checkbox
                    checked={fields.previousPeriod}
                    onCheckedChange={(checked) =>
                      set({ previousPeriod: !!checked })
                    }
                  />
                  Previous period
                </label>
              )}
            </div>

            <SectionTitle
              hint={
                <label
                  className={cn(
                    "flex items-center gap-1",
                    liveRange ? "cursor-pointer" : "cursor-not-allowed",
                  )}
                  title={
                    liveRange
                      ? "Query again at a regular interval"
                      : 'Only available when the range ends with "now"'
                  }
                >
                  <span>Auto refresh</span>
                  <select
                    value={autoRefresh}
                    disabled={!liveRange}
                    onChange={(event) => setAutoRefresh(Number(event.target.value))}
                    className="h-8 cursor-pointer rounded-md border border-input bg-card px-1.5 text-xs disabled:cursor-not-allowed disabled:opacity-50"
                  >
                    {refreshIntervals.map((interval) => (
                      <option key={interval.value} value={interval.value}>
                        {interval.label}
                      </option>
                    ))}
                  </select>
                </label>
              }
              action={
                <button
                  type="button"
                  title="Zoom out"
                  aria-label="Zoom out"
                  disabled={timeErrors}
                  className={iconButton}
                  onClick={zoomOut}
                >
                  <ZoomOut className="size-4" />
                </button>
              }
            >
              Time range
            </SectionTitle>
            <div className="grid grid-cols-2 gap-2 lg:grid-cols-1">
              <Combobox
                label="Presets"
                className="col-span-full"
                items={presets}
                itemKey={(preset) => preset.name}
                itemText={(preset) => preset.name}
                selected={(preset) => preset === selectedPreset}
                onSelect={(preset) => {
                  if (preset.start && preset.end) {
                    set({ humanStart: preset.start, humanEnd: preset.end });
                  }
                }}
                trigger={selectedPreset.name}
                placeholder="Search presets…"
              />
              <TextField
                label="Start"
                value={fields.humanStart}
                onChange={(humanStart) => set({ humanStart })}
                error={startError}
                button={shiftButton(-1, "Move the time range backward")}
              />
              <TextField
                label="End"
                value={fields.humanEnd}
                onChange={(humanEnd) => set({ humanEnd })}
                error={endError}
                button={shiftButton(1, "Move the time range forward")}
              />
            </div>

            <SectionTitle
              action={
                <button
                  type="button"
                  title="Reverse direction"
                  aria-label="Reverse the direction of the dimensions"
                  disabled={!!dimensionsError}
                  className={iconButton}
                  onClick={reverseDimensions}
                >
                  <ArrowLeftRight className="size-4" />
                </button>
              }
            >
              Dimensions
            </SectionTitle>
            <div className="grid grid-cols-2 gap-2">
              <DimensionsInput
                className="col-span-full"
                available={config?.dimensions ?? []}
                selected={fields.dimensions}
                onChange={(dimensions) => set({ dimensions })}
                error={dimensionsError}
              />
              {canAggregate && (
                <>
                  <TextField
                    label="IPv4 /x"
                    value={fields.truncate4}
                    onChange={(truncate4) => set({ truncate4 })}
                    error={truncate4Error}
                  />
                  <TextField
                    label="IPv6 /x"
                    value={fields.truncate6}
                    onChange={(truncate6) => set({ truncate6 })}
                    error={truncate6Error}
                  />
                </>
              )}
              <TextField
                label="Limit"
                value={fields.limit}
                onChange={(limit) => set({ limit })}
                error={limitError}
              />
              <Field label="Top by">
                <Select
                  value={fields.limitType}
                  onValueChange={(limitType) =>
                    set({ limitType: limitType as string })
                  }
                  items={limitTypes}
                >
                  <SelectTrigger aria-label="Top by" className="w-full bg-card">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {limitTypes.map((type) => (
                      <SelectItem key={type.value} value={type.value}>
                        {type.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </Field>
            </div>

            <SectionTitle
              action={
                <button
                  type="button"
                  title="Reverse direction"
                  aria-label="Reverse the direction of the filter"
                  disabled={filterInvalid}
                  className={iconButton}
                  onClick={reverseFilter}
                >
                  <ArrowLeftRight className="size-4" />
                </button>
              }
            >
              Filter
            </SectionTitle>
            <FilterInput
              value={fields.filter}
              onChange={(filter) => set({ filter })}
              onError={setFilterInvalid}
              onSubmit={apply}
            />
      </div>
      {/* Out of the scrolling part: the button stays in view. */}
      <div className="flex shrink-0 items-center gap-3 border-t px-4 py-3">
        <Button
          type="submit"
          variant={loading ? "outline" : "default"}
          disabled={hasErrors && !loading}
          className="h-10 grow"
        >
          {loading ? "Cancel" : applyLabel}
        </Button>
        {wide && (
          <kbd className="rounded border bg-card px-1.5 py-0.5 font-mono text-xs text-muted-foreground">
            Ctrl+Enter
          </kbd>
        )}
      </div>
    </form>
  );

  if (!wide) {
    return (
      <>
        <div className="order-last flex shrink-0 items-center gap-2 border-t bg-surface-2 px-4 py-2.5 print:hidden">
          <Button
            type="button"
            variant="outline"
            aria-haspopup="dialog"
            className="h-11 grow bg-card"
            onClick={() => setSheetOpen(true)}
          >
            <SlidersHorizontal />
            Options
          </Button>
          <Button
            type="button"
            variant={loading ? "outline" : "default"}
            disabled={hasErrors && !loading}
            className="h-11 grow"
            onClick={() => submit()}
          >
            {loading ? "Cancel" : applyLabel}
          </Button>
        </div>
        <Sheet open={sheetOpen} onOpenChange={setSheetOpen}>
          <SheetContent
            side="bottom"
            className="max-h-[85dvh] gap-0 rounded-t-3xl bg-surface-2"
          >
            <SheetHeader className="pb-0">
              <SheetTitle>Options</SheetTitle>
            </SheetHeader>
            {form}
          </SheetContent>
        </Sheet>
      </>
    );
  }

  return (
    <aside
      className={cn(
        "relative flex shrink-0 flex-col border-r bg-surface-2 transition-[width] duration-100 print:hidden",
        open ? "w-80" : "w-5",
      )}
    >
      <button
        type="button"
        aria-label={open ? "Hide options" : "Show options"}
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="absolute -right-3 top-4 z-10 flex size-6 cursor-pointer items-center justify-center rounded-full border bg-card text-muted-foreground shadow-card hover:text-foreground"
      >
        <ChevronRight
          className={cn("size-4 transition-transform", open && "rotate-180")}
        />
      </button>
      {open && form}
    </aside>
  );
}
