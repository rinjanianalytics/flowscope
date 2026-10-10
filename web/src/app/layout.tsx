// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

import type { Metadata } from "next";
import "@fontsource/poppins/400.css";
import "@fontsource/poppins/500.css";
import "@fontsource/poppins/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./globals.css";
import { Providers } from "@/components/providers";
import { AppSidebar, TopBar } from "@/components/navigation-bar";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";

export const metadata: Metadata = {
  title: "FlowScope",
  description: "Flow collector, enricher and visualizer",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en" className="h-full antialiased" suppressHydrationWarning>
      <body className="h-full">
        <Providers>
          <SidebarProvider className="h-full min-h-0">
            <AppSidebar />
            <SidebarInset className="flex h-full min-w-0 flex-col overflow-hidden print:block">
              <TopBar />
              <main className="relative flex min-h-0 grow overflow-y-auto">
                {children}
              </main>
            </SidebarInset>
          </SidebarProvider>
        </Providers>
      </body>
    </html>
  );
}
