// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  test: { include: ["src/**/*.test.ts"] },
});
