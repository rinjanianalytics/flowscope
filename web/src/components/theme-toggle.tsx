// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { Moon, Sun } from "lucide-react";
import { useTheme } from "next-themes";
import { Button } from "@/components/ui/button";
import { useIsDark } from "@/components/providers";

export function ThemeToggle() {
  const { setTheme } = useTheme();
  const isDark = useIsDark();
  return (
    <Button
      variant="ghost"
      size="icon"
      aria-label={isDark ? "Use the light theme" : "Use the dark theme"}
      onClick={() => setTheme(isDark ? "light" : "dark")}
    >
      {isDark ? <Sun /> : <Moon />}
    </Button>
  );
}
