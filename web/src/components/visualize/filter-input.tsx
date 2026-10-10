// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { useEffect, useRef } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Eye, EyeOff, Trash2 } from "lucide-react";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView, keymap, placeholder } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { history, standardKeymap } from "@codemirror/commands";
import { forceLinting, linter } from "@codemirror/lint";
import { acceptCompletion, autocompletion } from "@codemirror/autocomplete";
import { tags as t } from "@lezer/highlight";
import {
  filterCompletion,
  filterLanguage,
  filterLinterSource,
} from "@/lib/codemirror/lang-filter";
import { Button } from "@/components/ui/button";
import { Combobox, Field } from "@/components/visualize/field";
import { useIsDark, useUser } from "@/components/providers";
import { apiFetch, type SavedFilter } from "@/lib/api";
import { cn } from "@/lib/utils";

/** Colors of the filter language, from the brand kit. */
const highlight = (isDark: boolean) =>
  syntaxHighlighting(
    HighlightStyle.define([
      { tag: t.propertyName, color: isDark ? "#7fc9bc" : "#2b625b" },
      { tag: t.string, color: isDark ? "#e8a04a" : "#9a5d0c" },
      { tag: t.comment, color: isDark ? "#a3adb6" : "#5a6270" },
      { tag: t.operator, color: isDark ? "#8eaaf0" : "#3b5cc0" },
    ]),
  );

/**
 * The filter editor. The expression is checked by the server while typing.
 * `onError` tells if the current expression is invalid.
 */
export function FilterInput({
  value,
  onChange,
  onError,
  onSubmit,
}: {
  value: string;
  onChange: (value: string) => void;
  onError: (invalid: boolean) => void;
  onSubmit: () => void;
}) {
  const isDark = useIsDark();
  const user = useUser();
  const queryClient = useQueryClient();
  const element = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const theme = useRef(new Compartment());
  const callbacks = useRef({ onChange, onError, onSubmit });
  useEffect(() => {
    callbacks.current = { onChange, onError, onSubmit };
  });
  // Text the last validation applies to, and a submission waiting for it.
  const validated = useRef<string | null>(null);
  const pendingSubmit = useRef(false);

  // Saved filters
  const { data: saved } = useQuery({
    queryKey: ["saved-filters"],
    queryFn: () => apiFetch<{ filters: SavedFilter[] }>("filter/saved"),
  });
  const refresh = () =>
    queryClient.invalidateQueries({ queryKey: ["saved-filters"] });
  const deleteFilter = (id: number) =>
    apiFetch(`filter/saved/${id}`, { method: "DELETE" })
      .catch(() => {})
      .finally(refresh);
  const addFilter = (description: string, shared: boolean) =>
    apiFetch("filter/saved", {
      json: {
        description,
        shared,
        content: view.current?.state.doc.toString() ?? value,
      },
    })
      .catch(() => {})
      .finally(refresh);

  // Create the editor once.
  useEffect(() => {
    if (!element.current) return;
    // When the text is not validated yet, wait for the result instead of
    // dropping the submission.
    const submit = (v: EditorView) => {
      if (v.state.doc.toString() === validated.current) {
        callbacks.current.onSubmit();
      } else {
        pendingSubmit.current = true;
        forceLinting(v);
      }
      return true;
    };
    const instance = new EditorView({
      parent: element.current,
      state: EditorState.create({
        doc: value,
        extensions: [
          filterLanguage(),
          filterCompletion(),
          autocompletion({ icons: false }),
          linter(async (v) => {
            const code = v.state.doc.toString();
            try {
              const diagnostics = await filterLinterSource(v);
              if (code === v.state.doc.toString()) {
                callbacks.current.onError(diagnostics.length > 0);
                validated.current = code;
              }
              return diagnostics;
            } finally {
              if (pendingSubmit.current && code === v.state.doc.toString()) {
                pendingSubmit.current = false;
                // Let the error state settle first.
                setTimeout(() => callbacks.current.onSubmit(), 0);
              }
            }
          }),
          keymap.of([
            ...standardKeymap.filter((b) => b.key !== "Mod-a"),
            { key: "Tab", run: acceptCompletion },
            { key: "Ctrl-Enter", run: submit },
            { key: "Cmd-Enter", run: submit },
          ]),
          history(),
          placeholder("Filter expression"),
          EditorView.lineWrapping,
          theme.current.of([]),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) {
              callbacks.current.onChange(update.state.doc.toString());
              pendingSubmit.current = false;
            }
            if (update.focusChanged && !update.view.hasFocus) {
              // Trim the spaces at the end.
              const index = update.state.doc.toString().search(/\s+$/);
              if (index !== -1) {
                update.view.dispatch({
                  changes: { from: index, to: update.state.doc.length },
                });
              }
            }
          }),
        ],
      }),
    });
    view.current = instance;
    return () => {
      instance.destroy();
      view.current = null;
    };
    // The editor keeps its own copy of the text: see the next effect.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Follow the value when it changes from outside.
  useEffect(() => {
    const instance = view.current;
    if (instance && value !== instance.state.doc.toString()) {
      instance.dispatch({
        changes: { from: 0, to: instance.state.doc.length, insert: value },
      });
    }
  }, [value]);

  useEffect(() => {
    view.current?.dispatch({
      effects: theme.current.reconfigure([
        highlight(isDark),
        EditorView.theme({}, { dark: isDark }),
      ]),
    });
  }, [isDark]);

  const filters = saved?.filters ?? [];
  return (
    <>
      <Field label="Filter">
        <div
          ref={element}
          className={cn(
            "overflow-hidden rounded-lg border border-input bg-card transition-colors focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50",
          )}
        />
      </Field>
      <Combobox
        label="Saved filters"
        items={filters}
        itemKey={(filter) => filter.id.toString()}
        itemText={(filter) => filter.description}
        selected={() => false}
        onSelect={(filter) => onChange(filter.content)}
        placeholder="Search, or type a name to save…"
        trigger={<span className="text-muted-foreground">Load or save…</span>}
        renderItem={(filter) => (
          <>
            <span className="min-w-0 grow truncate">
              {filter.description}
              {filter.shared && filter.user !== user?.login && (
                <span className="block text-xs italic text-muted-foreground">
                  {filter.user === "__system"
                    ? "System filter"
                    : `Shared by ${filter.user}`}
                </span>
              )}
            </span>
            {filter.user === user?.login && (
              <span
                role="button"
                aria-label={`Delete ${filter.description}`}
                className="cursor-pointer rounded text-muted-foreground hover:text-destructive"
                onPointerDown={(event) => event.stopPropagation()}
                onClick={(event) => {
                  event.stopPropagation();
                  event.preventDefault();
                  deleteFilter(filter.id);
                }}
              >
                <Trash2 className="size-4" />
              </span>
            )}
          </>
        )}
        empty={(query, close) =>
          query.trim() ? (
            <div className="flex items-center justify-between gap-2">
              <span className="min-w-0 grow truncate">
                Save as “{query.trim()}”…
              </span>
              <span className="flex shrink-0 items-center gap-1">
                <Button
                  type="button"
                  variant="outline"
                  size="icon-sm"
                  title="Share with others"
                  onClick={() => {
                    addFilter(query.trim(), true);
                    close();
                  }}
                >
                  <Eye />
                </Button>
                <Button
                  type="button"
                  size="icon-sm"
                  title="Keep private"
                  onClick={() => {
                    addFilter(query.trim(), false);
                    close();
                  }}
                >
                  <EyeOff />
                </Button>
              </span>
            </div>
          ) : (
            "No saved filter."
          )
        }
      />
    </>
  );
}
