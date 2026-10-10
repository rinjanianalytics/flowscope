// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { useScrollFade } from "@/hooks/use-scroll-fade";
import Link from "next/link";
import { usePathname } from "next/navigation";
import {
  BookOpen,
  ChartArea,
  House,
  Menu,
  PanelLeftClose,
  PanelLeftOpen,
} from "lucide-react";
import { Wordmark } from "@/components/logo";
import { ThemeToggle } from "@/components/theme-toggle";
import { UserMenu } from "@/components/user-menu";
import { Button } from "@/components/ui/button";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "@/components/ui/sidebar";
import { useServerConfig } from "@/components/providers";

const navigation = [
  {
    group: "Monitor",
    items: [
      { name: "Home", icon: House, href: "/", match: (p: string) => p === "/" },
      {
        name: "Visualize",
        icon: ChartArea,
        href: "/visualize",
        match: (p: string) => p.startsWith("/visualize"),
      },
    ],
  },
  {
    group: "Learn",
    items: [
      {
        name: "Documentation",
        icon: BookOpen,
        href: "/docs",
        match: (p: string) => p.startsWith("/docs"),
      },
    ],
  },
];

const activeItem =
  "relative h-9 rounded-xl font-medium data-active:bg-sidebar-primary data-active:text-sidebar-primary-foreground data-active:hover:bg-sidebar-primary data-active:hover:text-sidebar-primary-foreground";

function CollapseButton() {
  const { open, isMobile, toggleSidebar } = useSidebar();
  if (isMobile) return null;
  return (
    <SidebarMenu className="gap-0">
      <SidebarMenuItem>
        <SidebarMenuButton
          onClick={toggleSidebar}
          aria-expanded={open}
          aria-label={open ? "Close sidebar" : "Open sidebar"}
          tooltip="Open (Ctrl+B)"
          className="h-8 rounded-xl text-muted-foreground hover:text-sidebar-accent-foreground"
        >
          {open ? <PanelLeftClose /> : <PanelLeftOpen />}
          <span className="truncate group-data-[collapsible=icon]:sr-only">
            Close
          </span>
        </SidebarMenuButton>
      </SidebarMenuItem>
    </SidebarMenu>
  );
}

/** The sidebar with the pages of the application. */
export function AppSidebar() {
  const pathname = usePathname();
  const config = useServerConfig();
  const { isMobile, setOpenMobile } = useSidebar();
  const navRef = useScrollFade<HTMLDivElement>();
  return (
    <Sidebar collapsible="icon" className="print:hidden">
      <SidebarHeader className="px-4 pt-4 transition-[padding] group-data-[collapsible=icon]:px-2">
        <Link href="/" aria-label="FlowScope home">
          <Wordmark
            className="group-data-[collapsible=icon]:justify-center"
            wordmarkClassName="group-data-[collapsible=icon]:hidden"
          />
        </Link>
      </SidebarHeader>
      {/* The navigation scrolls in both states (shadcn hides the overflow in
          the icon rail), with no visible scrollbar. */}
      <SidebarContent
        ref={navRef}
        className="no-scrollbar scroll-fade gap-0 pb-6 group-data-[collapsible=icon]:overflow-x-hidden group-data-[collapsible=icon]:overflow-y-auto"
      >
        {navigation.map((group) => (
          <SidebarGroup
            key={group.group}
            className="py-1 not-first:group-data-[collapsible=icon]:mt-1 not-first:group-data-[collapsible=icon]:border-t not-first:group-data-[collapsible=icon]:border-sidebar-border not-first:group-data-[collapsible=icon]:pt-2"
          >
            {/* In the icon rail, the hidden heading is pulled over the row
                above it: it must not take its clicks. */}
            <SidebarGroupLabel className="text-2xs font-semibold uppercase tracking-label text-muted-foreground group-data-[collapsible=icon]:pointer-events-none group-data-[collapsible=icon]:overflow-hidden">
              {group.group}
            </SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu className="gap-0">
                {group.items.map((item) => (
                  <SidebarMenuItem key={item.name}>
                    <SidebarMenuButton
                      isActive={item.match(pathname)}
                      tooltip={item.name}
                      className={activeItem}
                      onClick={() => isMobile && setOpenMobile(false)}
                      render={<Link href={item.href} />}
                    >
                      <item.icon />
                      <span className="truncate group-data-[collapsible=icon]:sr-only">
                        {item.name}
                      </span>
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                ))}
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        ))}
      </SidebarContent>
      <SidebarFooter className="border-t border-sidebar-border px-2 py-3">
        {config?.version && (
          <p className="truncate px-2 font-mono text-2xs text-muted-foreground group-data-[collapsible=icon]:hidden">
            {config.version}
          </p>
        )}
        <CollapseButton />
      </SidebarFooter>
    </Sidebar>
  );
}

/** The bar above the pages. */
export function TopBar() {
  const { setOpenMobile, openMobile } = useSidebar();
  return (
    <header className="sticky top-0 z-20 flex-none border-b bg-background/90 backdrop-blur-md print:hidden">
      <div className="flex min-w-0 items-center gap-2 px-4 py-2.5 sm:px-6">
        <Button
          variant="outline"
          size="icon"
          onClick={() => setOpenMobile(true)}
          aria-label="Open menu"
          aria-expanded={openMobile}
          className="size-10 shrink-0 bg-card md:hidden"
        >
          <Menu />
        </Button>
        <div id="topbar-slot" className="flex min-w-0 grow items-center gap-2" />
        <ThemeToggle />
        <UserMenu />
      </div>
    </header>
  );
}
