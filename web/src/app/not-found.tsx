// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

import type { Metadata } from "next";
import { NotFound } from "@/components/not-found";

export const metadata: Metadata = {
  title: "FlowScope | Not found",
};

export default function NotFoundPage() {
  return <NotFound />;
}
