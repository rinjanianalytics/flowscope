// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import Link from "next/link";
import { ErrorPage } from "@/components/error-page";
import { buttonVariants } from "@/components/ui/button";

/**
 * What to show for an address which leads nowhere, with two ways out. The
 * documentation follows its own links: it gives `onDocumentation`.
 */
export function NotFound({
  what = "page",
  onDocumentation,
}: {
  what?: "page" | "document";
  onDocumentation?: () => void;
}) {
  return (
    <ErrorPage
      code="Error 404"
      error={`This ${what} does not exist`}
      description={`The address may be mistyped, or the ${what} has moved.`}
    >
      <Link href="/" className={buttonVariants({ className: "h-11 px-5" })}>
        Back to home
      </Link>
      <Link
        href="/docs"
        onClick={
          onDocumentation &&
          ((event) => {
            event.preventDefault();
            onDocumentation();
          })
        }
        className={buttonVariants({
          variant: "outline",
          className: "h-11 bg-card px-5",
        })}
      >
        Open the documentation
      </Link>
    </ErrorPage>
  );
}
