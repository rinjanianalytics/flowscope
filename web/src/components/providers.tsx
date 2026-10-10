// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { usePathname, useRouter } from "next/navigation";
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
} from "@tanstack/react-query";
import { ThemeProvider, useTheme } from "next-themes";
import { TooltipProvider } from "@/components/ui/tooltip";
import {
  ApiError,
  apiFetch,
  type ServerConfig,
  type UserInfo,
} from "@/lib/api";

const ServerConfigContext = createContext<ServerConfig | null>(null);
const UserContext = createContext<UserInfo | null>(null);
const TitleContext = createContext<(title: string | null) => void>(() => {});

/** Configuration of the console, null while loading. */
export const useServerConfig = () => useContext(ServerConfigContext);

/** Current user, null while loading. */
export const useUser = () => useContext(UserContext);

const subscribeNothing = () => () => {};

/** Tell if the dark theme is displayed. */
export const useIsDark = () => {
  const { resolvedTheme } = useTheme();
  // The theme is only known in the browser.
  const mounted = useSyncExternalStore(
    subscribeNothing,
    () => true,
    () => false,
  );
  return mounted && resolvedTheme === "dark";
};

/** Set the part of the title describing the current document. */
export const useDocumentTitle = (title: string | null) => {
  const set = useContext(TitleContext);
  useEffect(() => {
    set(title);
    return () => set(null);
  }, [set, title]);
};

const viewNames: [string, string][] = [
  ["/visualize", "Visualize"],
  ["/docs", "Documentation"],
  ["/login", "Not authorized"],
];

/** Pages which do not need a user. */
const isPublicPath = (pathname: string) => pathname.startsWith("/login");

function AppState({ children }: { children: ReactNode }) {
  const pathname = usePathname();
  const router = useRouter();
  const publicPath = isPublicPath(pathname);

  const { data: config } = useQuery({
    queryKey: ["configuration"],
    queryFn: () => apiFetch<ServerConfig>("configuration"),
    staleTime: Infinity,
    enabled: !publicPath,
  });

  // The user is checked again on each page.
  const { data: user, error: userError } = useQuery({
    queryKey: ["user", pathname],
    queryFn: () => apiFetch<UserInfo>("user/info"),
    enabled: !publicPath,
    retry: false,
    placeholderData: (previous) => previous,
  });
  useEffect(() => {
    if (userError instanceof ApiError && userError.status === 401) {
      router.replace(`/login?redirect=${encodeURIComponent(pathname)}`);
    }
  }, [userError, router, pathname]);

  // The title has three parts: the application, the view and the document.
  const [documentTitle, setDocumentTitle] = useState<string | null>(null);
  const viewName = useMemo(
    () =>
      pathname === "/"
        ? "Home"
        : (viewNames.find(([prefix]) => pathname.startsWith(prefix))?.[1] ??
          "Not found"),
    [pathname],
  );
  useEffect(() => {
    document.title = ["FlowScope", viewName, documentTitle]
      .filter((part) => !!part)
      .join(" | ");
  }, [viewName, documentTitle]);

  return (
    <ServerConfigContext.Provider value={config ?? null}>
      <UserContext.Provider value={user ?? null}>
        <TitleContext.Provider value={setDocumentTitle}>
          {children}
        </TitleContext.Provider>
      </UserContext.Provider>
    </ServerConfigContext.Provider>
  );
}

export function Providers({ children }: { children: ReactNode }) {
  const [queryClient] = useState(
    () =>
      new QueryClient({
        defaultOptions: {
          queries: { refetchOnWindowFocus: false, retry: 1 },
        },
      }),
  );
  return (
    <QueryClientProvider client={queryClient}>
      <ThemeProvider
        attribute="class"
        defaultTheme="system"
        enableSystem
        disableTransitionOnChange
      >
        <TooltipProvider delay={300}>
          <AppState>{children}</AppState>
        </TooltipProvider>
      </ThemeProvider>
    </QueryClientProvider>
  );
}
