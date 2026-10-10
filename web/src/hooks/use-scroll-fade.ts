// SPDX-FileCopyrightText: 2026 FlowScope contributors
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { useCallback } from "react";

/**
 * Marks a scroll container with data-fade-top / data-fade-bottom while its
 * content runs past that edge, for the `scroll-fade` utility. Used where the
 * scrollbar is hidden (`no-scrollbar`), so people still see there is more to
 * scroll to. It updates on scroll and whenever the container or its content
 * changes size (sidebar open and close, groups expanding).
 *
 * The result is a ref callback, so it also works for an element which comes
 * and goes.
 */
export function useScrollFade<T extends HTMLElement>() {
  return useCallback((el: T | null) => {
    if (!el) return;
    const update = () => {
      el.toggleAttribute("data-fade-top", el.scrollTop > 1);
      el.toggleAttribute(
        "data-fade-bottom",
        el.scrollTop + el.clientHeight < el.scrollHeight - 1,
      );
    };
    update();
    el.addEventListener("scroll", update, { passive: true });
    const sizes = new ResizeObserver(update);
    sizes.observe(el);
    for (const child of el.children) sizes.observe(child);
    // Children are added later too (a list filled once its data is there).
    const children = new MutationObserver(() => {
      for (const child of el.children) sizes.observe(child);
      update();
    });
    children.observe(el, { childList: true });
    return () => {
      el.removeEventListener("scroll", update);
      sizes.disconnect();
      children.disconnect();
    };
  }, []);
}
