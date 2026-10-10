import type { NextConfig } from "next";

// The console can be hosted under any URL prefix, chosen at run time. The
// static export is built with this placeholder as base path and the
// FlowScope console replaces it with the configured prefix when serving the
// files.
const basePlaceholder = "/__flowscope_base__";
const production = process.env.NODE_ENV === "production";

// Console API used by the development server.
const apiTarget = process.env.FLOWSCOPE_API_URL || "http://127.0.0.1:8080";

const nextConfig: NextConfig = {
  output: production ? "export" : undefined,
  basePath: production ? basePlaceholder : undefined,
  env: {
    NEXT_PUBLIC_BASE_PATH: production ? basePlaceholder : "",
  },
  images: { unoptimized: true },
  turbopack: {
    rules: {
      "*.css": {
        loaders: ["@tailwindcss/turbopack"],
        as: "*.css",
      },
    },
  },
  async rewrites() {
    if (production) return [];
    return [
      { source: "/api/:path*", destination: `${apiTarget}/api/:path*` },
      { source: "/assets/docs/:path*", destination: `${apiTarget}/assets/docs/:path*` },
      // The pages with a parameter in their path are served by their parent,
      // like the console does.
      { source: "/docs/:id", destination: "/docs" },
      { source: "/visualize/:state", destination: "/visualize" },
    ];
  },
};

export default nextConfig;
