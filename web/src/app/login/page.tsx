// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

import { ErrorPage } from "@/components/error-page";

export default function NotAuthorized() {
  return <ErrorPage code="Error 401" error="Not authorized!" />;
}
