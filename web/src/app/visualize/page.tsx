// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { CircleAlert, GripHorizontal, LoaderCircle } from "lucide-react";
import { PageHeader, Panel } from "@/components/brand";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { DataGraph } from "@/components/visualize/graphs";
import { DataTable } from "@/components/visualize/data-table";
import { OptionsPanel } from "@/components/visualize/options-panel";
import { RequestSummary } from "@/components/visualize/request-summary";
import {
  apiUrl,
  appPath,
  appUrl,
  type GraphLineInput,
  type GraphLineOutput,
  type GraphResult,
  type GraphSankeyInput,
  type GraphSankeyOutput,
} from "@/lib/api";
import {
  decodeState,
  encodeState,
  isEqual,
  type VisualizeState,
} from "@/lib/state";
import { cn } from "@/lib/utils";

/** The state stored in the URL: `?s=`, or the path of older links. */
const stateFromLocation = () => {
  const param = new URLSearchParams(window.location.search).get("s");
  if (param) return decodeState(param);
  const match = appPath().match(/^\/visualize\/(.+)$/);
  return match ? decodeState(decodeURIComponent(match[1])) : null;
};

/** The body of the query, with its keys in order to help caches. */
const payload = (state: VisualizeState): GraphLineInput | GraphSankeyInput => {
  const {
    graphType,
    previousPeriod,
    humanStart: _humanStart,
    humanEnd: _humanEnd,
    ...rest
  } = state;
  const input =
    graphType === "sankey"
      ? rest
      : {
          ...rest,
          points: graphType === "grid" ? 50 : 200,
          "previous-period": previousPeriod,
        };
  return Object.fromEntries(
    Object.entries(input).sort(([a], [b]) => a.localeCompare(b)),
  ) as GraphLineInput | GraphSankeyInput;
};

const logQuery = (response: Response, title: string) => {
  const query = response.headers.get("x-sql-query");
  if (!query) return;
  console.groupCollapsed(title);
  console.info(query);
  console.groupEnd();
};

export default function VisualizePage() {
  const [ready, setReady] = useState(false);
  const [state, setState] = useState<VisualizeState | null>(null);
  // The state once its query succeeded, and the matching result.
  const [request, setRequest] = useState<VisualizeState | null>(null);
  const [result, setResult] = useState<GraphResult | null>(null);
  const [error, setError] = useState("");
  const [fetching, setFetching] = useState(false);
  const [highlight, setHighlight] = useState<number | null>(null);
  const [graphHeight, setGraphHeight] = useState(500);
  // The last change comes from the auto refresh.
  const auto = useRef(false);
  const [quiet, setQuiet] = useState(false);
  const controller = useRef<AbortController | null>(null);

  // Read the state from the URL, now and when going back or forward.
  useEffect(() => {
    const load = () => {
      const next = stateFromLocation();
      auto.current = false;
      setState((current) => (isEqual(current, next) ? current : next));
    };
    load();
    // The URL is only known in the browser.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setReady(true);
    window.addEventListener("popstate", load);
    return () => window.removeEventListener("popstate", load);
  }, []);

  const updateState = useCallback((value: VisualizeState, isAuto?: boolean) => {
    auto.current = !!isAuto;
    setQuiet(!!isAuto);
    setState(value);
  }, []);

  const updateTimeRange = useCallback(([start, end]: [Date, Date]) => {
    auto.current = false;
    setQuiet(false);
    setState((current) =>
      current
        ? {
            ...current,
            start: start.toISOString(),
            end: end.toISOString(),
            humanStart: start.toISOString(),
            humanEnd: end.toISOString(),
          }
        : current,
    );
  }, []);

  // Query on each change. The previous result is kept on errors.
  useEffect(() => {
    if (!state) return;
    const abort = new AbortController();
    controller.current = abort;
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setFetching(true);
    const endpoint = state.graphType === "sankey" ? "sankey" : "line";
    (async () => {
      try {
        const response = await fetch(apiUrl(`graph/${endpoint}`), {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(payload(state)),
          signal: abort.signal,
        });
        const data = await response.json().catch(() => null);
        if (!response.ok) {
          logQuery(response, "SQL query (with error)");
          setError(
            data?.message ??
              `Server returned an error: ${response.status} ${response.statusText}`,
          );
          return;
        }
        logQuery(response, "SQL query");
        const context = {
          start: state.start,
          end: state.end,
          dimensions: state.dimensions,
          units: state.units,
          bidirectional: state.bidirectional,
        };
        setResult(
          state.graphType === "sankey"
            ? {
                graphType: "sankey",
                ...(data as GraphSankeyOutput),
                ...context,
              }
            : {
                graphType: state.graphType,
                ...(data as GraphLineOutput),
                ...context,
              },
        );
        setRequest(state);
        setError("");
        // Keep the state in the URL. A new query is a new history entry,
        // except the first one and the automatic ones.
        const url = appUrl(`/visualize?s=${encodeState(state)}`);
        const hasState = stateFromLocation() !== null;
        if (!hasState || auto.current || window.location.search === "") {
          window.history.replaceState(null, "", url);
        } else if (!isEqual(stateFromLocation(), state)) {
          window.history.pushState(null, "", url);
        }
      } catch (e) {
        if (!abort.signal.aborted) {
          setError(`Server returned an error: ${e}`);
        }
      } finally {
        if (controller.current === abort) {
          controller.current = null;
          setFetching(false);
        }
      }
    })();
    return () => abort.abort();
  }, [state]);

  const cancel = useCallback(() => {
    controller.current?.abort();
    controller.current = null;
    setFetching(false);
  }, []);

  // Resize the graph by dragging the handle below it.
  const startResize = (event: React.PointerEvent) => {
    const startY = event.clientY;
    const startHeight = graphHeight;
    const move = (e: PointerEvent) =>
      setGraphHeight(Math.max(200, startHeight + e.clientY - startY));
    const stop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
  };

  const showOverlay = fetching && !quiet;
  return (
    <div className="flex size-full flex-col lg:flex-row">
      {ready && (
        <OptionsPanel
          value={state}
          loading={fetching}
          onChange={updateState}
          onCancel={cancel}
        />
      )}
      <div className="min-w-0 grow overflow-y-auto">
        <div className="flex flex-col gap-5 px-4 py-6 sm:px-6">
          <PageHeader
            eyebrow="Visualize"
            title="Explore your flows"
            description="Pick a time range, dimensions and a filter. Select a part of the graph with the mouse to zoom in."
          />
          <RequestSummary request={request} />
          {error && (
            <Alert variant="destructive">
              <CircleAlert />
              <AlertTitle>Unable to fetch data!</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          <Panel className="relative break-inside-avoid-page" contentClassName="gap-0">
            <div style={{ height: graphHeight }}>
              <DataGraph
                data={result}
                highlight={highlight}
                onTimeRange={updateTimeRange}
              />
            </div>
            <div
              role="separator"
              aria-orientation="horizontal"
              aria-label="Resize the graph"
              onPointerDown={startResize}
              className="-mb-3 mt-1 flex cursor-row-resize touch-none justify-center text-muted-foreground/50 hover:text-muted-foreground print:hidden"
            >
              <GripHorizontal className="size-4" />
            </div>
            <div
              className={cn(
                "pointer-events-none absolute inset-0 flex items-center justify-center rounded-3xl bg-card/60 opacity-0 transition-opacity",
                showOverlay && "pointer-events-auto opacity-100",
              )}
            >
              <LoaderCircle className="size-8 animate-spin text-primary" />
            </div>
          </Panel>
          {result && (
            <Panel
              eyebrow="Details"
              title="Series"
              className="break-inside-avoid-page"
              contentClassName="px-3"
            >
              <DataTable data={result} onHighlight={setHighlight} />
            </Panel>
          )}
        </div>
      </div>
    </div>
  );
}
