// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

// Client for the console API.

import type { components } from "./api-schema";
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

// The types of the API are generated from the OpenAPI document of the
// console (`pnpm run api`), so they cannot drift from the server.
type Schemas = components["schemas"];

export type ServerConfig = Schemas["Configuration"];
export type UserInfo = Schemas["UserInfo"];
export type SavedFilter = Schemas["SavedFilter"];
export type Units = Schemas["Units"];
export type GraphSankeyInput = Schemas["GraphInput"];
export type GraphLineInput = Schemas["LineInput"];
export type GraphSankeyOutput = Schemas["SankeyOutput"];
export type GraphLineOutput = Schemas["LineOutput"];
export type DocumentToc = Schemas["DocumentToc"];

type ResultContext = Required<
  Pick<GraphSankeyInput, "start" | "end" | "dimensions" | "units" | "bidirectional">
>;

export type GraphSankeyResult = GraphSankeyOutput &
  ResultContext & { graphType: "sankey" };
export type GraphLineResult = GraphLineOutput &
  ResultContext & { graphType: Exclude<GraphType, "sankey"> };
export type GraphResult = GraphLineResult | GraphSankeyResult;
