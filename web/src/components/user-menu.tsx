// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { LogOut } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { useUser } from "@/components/providers";
import { apiUrl } from "@/lib/api";

export function UserMenu() {
  const user = useUser();
  const name = user?.name || user?.email || user?.login || "Guest";
  const avatarUrl = user?.["avatar-url"] ?? apiUrl("user/avatar");
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        className="rounded-full outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
        aria-label="Open user menu"
      >
        <Avatar>
          {user && <AvatarImage src={avatarUrl} alt="" />}
          <AvatarFallback>{name.slice(0, 1).toUpperCase()}</AvatarFallback>
        </Avatar>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-48">
        <DropdownMenuGroup>
          <DropdownMenuLabel>
            <span className="block truncate text-sm text-foreground">
              {name}
            </span>
            {user?.name && user?.email && (
              <span className="block truncate text-xs font-normal text-muted-foreground">
                {user.email}
              </span>
            )}
          </DropdownMenuLabel>
        </DropdownMenuGroup>
        {user?.["logout-url"] && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              render={<a href={user["logout-url"]} />}
            >
              <LogOut />
              Logout
            </DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
