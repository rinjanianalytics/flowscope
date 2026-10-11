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

/** The current user, at the foot of the menu. */
export function UserMenu() {
  const user = useUser();
  const name = user?.name || user?.email || user?.login || "Guest";
  const avatarUrl = user?.["avatar-url"] ?? apiUrl("user/avatar");
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        className="flex min-w-0 grow cursor-pointer items-center gap-2.5 rounded-xl p-1 text-left outline-none hover:bg-sidebar-accent focus-visible:ring-3 focus-visible:ring-ring/50 group-data-[collapsible=icon]:grow-0"
        aria-label={`User menu of ${name}`}
      >
        <Avatar>
          {user && <AvatarImage src={avatarUrl} alt="" />}
          <AvatarFallback>{name.slice(0, 1).toUpperCase()}</AvatarFallback>
        </Avatar>
        <span className="min-w-0 leading-tight group-data-[collapsible=icon]:hidden">
          <span className="block truncate text-sm font-medium text-sidebar-accent-foreground">
            {name}
          </span>
          {user?.name && user?.email && (
            <span className="block truncate text-xs text-muted-foreground">
              {user.email}
            </span>
          )}
        </span>
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="start" className="min-w-48">
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
