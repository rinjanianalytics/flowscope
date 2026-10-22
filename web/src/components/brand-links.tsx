// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { Globe } from "lucide-react";
import {
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar";

export const WEBSITE = "https://rinjanianalytics.com";
export const SOURCE = "https://github.com/rinjanianalytics/flowscope";
const AKVORADO = "https://github.com/akvorado/akvorado";

/** The GitHub mark, from Octicons (MIT). Lucide has no brand icons. */
function GitHubMark(props: React.ComponentProps<"svg">) {
  return (
    <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true" {...props}>
      <path d="M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z" />
    </svg>
  );
}

const links = [
  { name: "GitHub", href: SOURCE, icon: GitHubMark },
  { name: "rinjanianalytics.com", href: WEBSITE, icon: Globe },
];

/** Links to the source and to Rinjani Analytics, at the foot of the menu. */
export function BrandLinks() {
  return (
    <nav aria-label="Rinjani Analytics">
      <SidebarMenu className="gap-0">
        {links.map((link) => (
          <SidebarMenuItem key={link.name}>
            <SidebarMenuButton
              tooltip={link.name}
              className="h-8 rounded-xl text-muted-foreground hover:text-sidebar-accent-foreground"
              render={<a href={link.href} target="_blank" rel="noreferrer" />}
            >
              <link.icon />
              <span className="truncate group-data-[collapsible=icon]:sr-only">
                {link.name}
              </span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        ))}
      </SidebarMenu>
    </nav>
  );
}

const textLink =
  "underline underline-offset-2 hover:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring rounded-sm";

/** Where FlowScope comes from, its license and its version. */
export function Credit({ version }: { version?: string }) {
  return (
    <p className="px-2 text-2xs leading-relaxed text-muted-foreground group-data-[collapsible=icon]:hidden">
      Based on{" "}
      <a href={AKVORADO} target="_blank" rel="noreferrer" className={textLink}>
        Akvorado
      </a>{" "}
      ·{" "}
      <a
        href={`${SOURCE}/blob/main/LICENSE.txt`}
        target="_blank"
        rel="noreferrer"
        className={textLink}
      >
        AGPL-3.0
      </a>
      {version && (
        <>
          <br />
          <span className="font-mono">{version}</span>
        </>
      )}
    </p>
  );
}
