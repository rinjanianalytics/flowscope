// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

// Client for the console API.

import type { GraphType } from "./graph-types";

/** URL prefix the console is hosted under, without trailing slash. */
export const basePath = process.env.NEXT_PUBLIC_BASE_PATH ?? "";

/** URL of an API endpoint ("graph/line"). */
export const apiUrl = (endpoint: string) =>
  `${basePath}/api/v0/console/${endpoint}`;

/** URL of a page of the application ("/docs/intro"). */
export const appUrl = (path: string) => `${basePath}${path}`;

/** Path of the current page inside the application. */
export const appPath = () => {
  const path = window.location.pathname;
  const stripped = path.startsWith(basePath) ? path.slice(basePath.length) : path;
  return stripped.replace(/\.html$/, "") || "/";
};

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

/** Fetch JSON from the API. Errors carry the message of the server. */
export async function apiFetch<T>(
  endpoint: string,
  init?: RequestInit & { json?: unknown },
): Promise<T> {
  const { json, ...rest } = init ?? {};
  const response = await fetch(apiUrl(endpoint), {
    ...rest,
    ...(json !== undefined
      ? {
          method: rest.method ?? "POST",
          body: JSON.stringify(json),
          headers: { "Content-Type": "application/json", ...rest.headers },
        }
      : {}),
  });
  if (!response.ok) {
    let message = `Server returned an error: ${response.status} ${response.statusText}`;
    try {
      const data = await response.json();
      if (data?.message) message = data.message;
    } catch {
      // Not JSON, keep the generic message.
    }
    throw new ApiError(response.status, message);
  }
  if (response.status === 204) return undefined as T;
  return response.json();
}

export type ServerConfig = {
  version: string;
  defaultVisualizeOptions: {
    graphType: GraphType;
    start: string;
    end: string;
    filter: string;
    dimensions: string[];
    limit: number;
    limitType: string;
    bidirectional: boolean;
    previousPeriod: boolean;
  };
  dimensions: string[];
  dimensionsLimit: number;
  truncatable: string[];
  homepageTopWidgets: string[];
  branding: boolean;
};

export type UserInfo = {
  login: string;
  name?: string;
  email?: string;
  "logout-url"?: string;
  "avatar-url"?: string;
};

export type SavedFilter = {
  id: number;
  user: string;
  shared: boolean;
  description: string;
  content: string;
};

export type Units = "l3bps" | "l2bps" | "fps" | "pps" | "inl2%" | "outl2%";

export type GraphSankeyInput = {
  start: string;
  end: string;
  dimensions: string[];
  limit: number;
  limitType: string;
  "truncate-v4": number;
  "truncate-v6": number;
  filter: string;
  units: Units;
  bidirectional: boolean;
};

export type GraphLineInput = GraphSankeyInput & {
  points: number;
  "previous-period": boolean;
};

export type GraphSankeyOutput = {
  rows: string[][];
  xps: number[];
  axis: number[];
  "axis-names": Record<number, string>;
  nodes: { name: string; axis: number }[];
  links: { source: string; target: string; xps: number; axis: number }[];
};

export type GraphLineOutput = {
  t: string[];
  rows: string[][];
  points: number[][];
  axis: number[];
  "axis-names": Record<number, string>;
  average: number[];
  total?: number[];
  min: number[];
  max: number[];
  last: number[];
  "95th": number[];
};

type ResultContext = Pick<
  GraphSankeyInput,
  "start" | "end" | "dimensions" | "units" | "bidirectional"
>;

export type GraphSankeyResult = GraphSankeyOutput &
  ResultContext & { graphType: "sankey" };
export type GraphLineResult = GraphLineOutput &
  ResultContext & { graphType: Exclude<GraphType, "sankey"> };
export type GraphResult = GraphLineResult | GraphSankeyResult;

export type DocumentToc = {
  name: string;
  section: string;
  headers: { level: number; id: string; title: string }[];
};
