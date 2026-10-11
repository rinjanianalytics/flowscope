// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

import type { Metadata } from "next";
import Link from "next/link";
import { ErrorPage } from "@/components/error-page";
import { buttonVariants } from "@/components/ui/button";

export const metadata: Metadata = {
  title: "FlowScope | Not found",
};

export default function NotFound() {
  return (
    <ErrorPage
      code="Error 404"
      error="This page does not exist"
      description="The address may be mistyped, or the page has moved."
    >
      <Link href="/" className={buttonVariants({ className: "h-11 px-5" })}>
        Back to home
      </Link>
      <Link
        href="/docs"
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
