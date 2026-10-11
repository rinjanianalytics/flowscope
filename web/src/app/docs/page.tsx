// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronRight, CircleAlert, GitBranch } from "lucide-react";
import { SectionLabel } from "@/components/brand";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { useDocumentTitle, useServerConfig } from "@/components/providers";
import { apiUrl, appPath, appUrl, basePath, type DocumentToc } from "@/lib/api";
import { cn } from "@/lib/utils";
import { useScrollFade } from "@/hooks/use-scroll-fade";

type Location = { id: string; hash: string };

/** The document and the anchor the URL points to. */
const currentLocation = (): Location => {
  const match = appPath().match(/^\/docs\/([^/]+)/);
  return { id: match ? match[1] : "", hash: window.location.hash };
};

type DocumentationAnswer = { markdown: string; toc: DocumentToc[] };

export default function DocumentationPage() {
  const config = useServerConfig();
  const [location, setLocation] = useState<Location | null>(null);
  const [expanded, setExpanded] = useState<{ id: string; names: Set<string> }>(
    { id: "", names: new Set() },
  );
  const content = useRef<HTMLDivElement>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const fadeRef = useScrollFade<HTMLElement>();

  // Follow the URL. The documentation root is the introduction.
  useEffect(() => {
    const load = () => {
      const next = currentLocation();
      if (!next.id) {
        window.history.replaceState(null, "", appUrl("/docs/intro"));
        next.id = "intro";
      }
      setLocation(next);
    };
    load();
    window.addEventListener("popstate", load);
    window.addEventListener("hashchange", load);
    return () => {
      window.removeEventListener("popstate", load);
      window.removeEventListener("hashchange", load);
    };
  }, []);
  const navigate = useCallback((id: string, hash: string) => {
    window.history.pushState(null, "", appUrl(`/docs/${id}${hash}`));
    setLocation({ id, hash });
  }, []);

  const id = location?.id;
  const { data, error } = useQuery({
    queryKey: ["docs", id],
    enabled: !!id,
    queryFn: async (): Promise<DocumentationAnswer> => {
      const response = await fetch(apiUrl(`docs/${id}`), {
        headers: { Accept: "application/json" },
      });
      const answer = await response.json().catch(() => null);
      if (!response.ok) {
        throw new Error(
          answer?.message ??
            `Server returned an error: ${response.status} ${response.statusText}`,
        );
      }
      return answer;
    },
    placeholderData: (previous) => previous,
    retry: false,
  });

  // The links and the images of a document are relative to the root of the
  // application.
  const html = useMemo(
    () =>
      (data?.markdown ?? "")
        .replaceAll('href="docs/', `href="${basePath}/docs/`)
        .replaceAll('src="assets/docs/', `src="${basePath}/assets/docs/`),
    [data],
  );
  const toc = data?.toc ?? [];

  // Only the current document is expanded when switching. The other ones
  // are toggled by the user.
  const toggle = (name: string) =>
    setExpanded((current) => {
      const next = new Set(current.id === id ? current.names : [id ?? ""]);
      if (!next.delete(name)) next.add(name);
      return { id: id ?? "", names: next };
    });
  const isExpanded = (name: string) =>
    expanded.id === id ? expanded.names.has(name) : name === id;

  // Scroll to the anchor once the document is displayed.
  const hash = location?.hash ?? "";
  useEffect(() => {
    if (!scroller.current || !html) return;
    const target = hash
      ? document.getElementById(decodeURIComponent(hash.slice(1)))
      : null;
    if (target) target.scrollIntoView();
    else scroller.current.scrollTo(0, 0);
  }, [html, hash]);

  // The title of the page is the title of the document.
  const [title, setTitle] = useState<string | null>(null);
  useEffect(() => {
    // The title comes from the rendered document.
    setTitle(content.current?.querySelector("h1")?.textContent ?? null);
  }, [html]);
  useDocumentTitle(title);

  // Hand over to the application the links it knows about, instead of
  // loading the whole page again.
  const followLink = (event: React.MouseEvent) => {
    if (
      event.defaultPrevented ||
      event.button !== 0 ||
      event.altKey ||
      event.ctrlKey ||
      event.metaKey ||
      event.shiftKey
    )
      return;
    const link = (event.target as Element).closest("a");
    if (!link || !link.getAttribute("href") || link.target) return;
    const url = new URL(link.href);
    if (url.origin !== window.location.origin) return;
    const prefix = `${basePath}/docs/`;
    if (!url.pathname.startsWith(prefix)) return;
    const name = url.pathname.slice(prefix.length);
    if (!name || name.includes("/")) return;
    event.preventDefault();
    navigate(name, url.hash);
  };

  const tocLink = (name: string, anchor: string, active: boolean, label: string) => (
    <a
      href={appUrl(`/docs/${name}#${anchor}`)}
      onClick={(event) => {
        event.preventDefault();
        navigate(name, `#${anchor}`);
      }}
      className={cn(
        "block rounded-md px-2 py-1 outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50",
        active ? "font-medium text-brand-ink" : "text-muted-foreground",
      )}
    >
      {label}
    </a>
  );

  return (
    <div className="flex size-full flex-col md:flex-row">
      <nav
        ref={fadeRef}
        aria-label="Documentation"
        className="no-scrollbar scroll-fade max-h-64 flex-none overflow-y-auto border-b bg-surface-2 px-3 py-4 text-ui md:max-h-none md:w-72 md:border-b-0 md:border-r print:hidden"
      >
        {config?.branding && (
          <a
            href="https://github.com/rinjanianalytics/flowscope"
            className="mb-3 flex items-center gap-2 border-b px-2 pb-3 text-muted-foreground hover:text-foreground"
          >
            <GitBranch className="size-4" />
            rinjanianalytics/flowscope
          </a>
        )}
        <ul className="flex flex-col gap-0.5">
          {toc.map((document, index) => {
            const first = document.headers[0];
            if (!first) return null;
            const open = isExpanded(document.name);
            const current = id === document.name;
            return (
              <li key={document.name}>
                {document.section &&
                  document.section !== toc[index - 1]?.section && (
                    <p className="mb-1 mt-4 px-2 text-2xs font-semibold uppercase tracking-label text-muted-foreground">
                      {document.section}
                    </p>
                  )}
                <div className="flex items-center">
                  <button
                    type="button"
                    aria-label={open ? "Collapse" : "Expand"}
                    aria-expanded={open}
                    onClick={() => toggle(document.name)}
                    className="flex size-6 flex-none cursor-pointer items-center justify-center rounded-md text-muted-foreground hover:text-foreground"
                  >
                    <ChevronRight
                      className={cn(
                        "size-3.5 transition-transform",
                        open && "rotate-90",
                      )}
                    />
                  </button>
                  <span className="min-w-0 grow">
                    <a
                      href={appUrl(`/docs/${document.name}#${first.id}`)}
                      onClick={(event) => {
                        event.preventDefault();
                        navigate(document.name, `#${first.id}`);
                      }}
                      aria-current={current ? "page" : undefined}
                      className={cn(
                        "block truncate rounded-xl px-2 py-1.5 font-medium outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/50",
                        // The tint of the brand, like the current page in
                        // the menu.
                        current
                          ? "bg-accent font-semibold text-accent-foreground"
                          : "text-foreground hover:bg-sidebar-accent",
                      )}
                    >
                      {first.title}
                    </a>
                  </span>
                </div>
                {open && (
                  <ul className="mb-2 ml-6 mt-1 flex flex-col border-l pl-1">
                    {document.headers
                      .filter((h) => h.level >= 2 && h.level <= 3)
                      .map((header) => (
                        <li
                          key={header.id}
                          className={cn(header.level === 3 && "ml-3")}
                        >
                          {tocLink(
                            document.name,
                            header.id,
                            current && hash.slice(1) === header.id,
                            header.title,
                          )}
                        </li>
                      ))}
                  </ul>
                )}
              </li>
            );
          })}
        </ul>
      </nav>
      <div ref={scroller} className="min-w-0 grow overflow-y-auto">
        <div className="mx-auto flex max-w-4xl flex-col gap-4 px-4 py-6 sm:px-8">
          <SectionLabel>Documentation</SectionLabel>
          {error && (
            <Alert variant="destructive">
              <CircleAlert />
              <AlertTitle>Unable to fetch documentation page!</AlertTitle>
              <AlertDescription>{error.message}</AlertDescription>
            </Alert>
          )}
          <div
            ref={content}
            onClick={followLink}
            className="prose prose-sm max-w-none pb-8 dark:prose-invert md:prose-base prose-headings:scroll-mt-4 prose-headings:font-semibold prose-headings:tracking-tight prose-h1:text-title prose-h1:tracking-display prose-a:text-brand-ink prose-code:font-mono prose-code:before:content-none prose-code:after:content-none prose-pre:whitespace-pre-wrap prose-pre:break-all prose-pre:rounded-2xl prose-img:mx-auto prose-img:rounded-2xl"
            dangerouslySetInnerHTML={{ __html: html }}
          />
        </div>
      </div>
    </div>
  );
}
