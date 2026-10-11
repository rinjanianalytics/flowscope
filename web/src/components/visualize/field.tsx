// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import { useId, useState, type ReactNode } from "react";
import { Check, ChevronsUpDown } from "lucide-react";
import { Input } from "@/components/ui/input";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { cn } from "@/lib/utils";

/** A label above a field, with its error below. */
export function Field({
  label,
  error,
  htmlFor,
  labelId,
  className,
  children,
}: {
  label: string;
  error?: string;
  htmlFor?: string;
  /** For a field which is not an input: it names itself with this label. */
  labelId?: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div className={cn("flex min-w-0 flex-col gap-1", className)}>
      <label
        id={labelId}
        htmlFor={htmlFor}
        className="text-ui text-muted-foreground"
      >
        {label}
      </label>
      {children}
      {error && <p className="text-xs text-destructive">{error}</p>}
    </div>
  );
}

/** A text input with a label and an optional button on its right. */
export function TextField({
  label,
  value,
  onChange,
  error,
  button,
  className,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  error?: string;
  button?: ReactNode;
  className?: string;
}) {
  const id = useId();
  return (
    <Field label={label} error={error} htmlFor={id} className={className}>
      <div className="relative">
        <Input
          id={id}
          value={value}
          aria-invalid={!!error}
          onChange={(event) => onChange(event.target.value)}
          className={cn("bg-card text-sm", button && "pr-9")}
        />
        {button && (
          <span className="absolute inset-y-0 right-0 flex items-center">
            {button}
          </span>
        )}
      </div>
    </Field>
  );
}

/**
 * A section title in the options panel, with an action next to it and a hint
 * on its right.
 */
export function SectionTitle({
  children,
  action,
  hint,
}: {
  children: ReactNode;
  action?: ReactNode;
  hint?: ReactNode;
}) {
  return (
    <div className="mt-2 flex min-h-8 items-center justify-between gap-2 text-muted-foreground">
      <span className="flex items-center gap-1">
        <h2 className="text-2xs font-semibold uppercase tracking-label">
          {children}
        </h2>
        {action}
      </span>
      {hint && <span className="text-xs">{hint}</span>}
    </div>
  );
}

// 32 pixels wide: an icon alone is too small a target.
export const iconButton =
  "inline-flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted-foreground outline-none hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:bg-transparent disabled:hover:text-muted-foreground";

/** A searchable list of choices in a popover. */
export function Combobox<T>({
  label,
  items,
  itemKey,
  itemText,
  renderItem,
  selected,
  onSelect,
  trigger,
  placeholder = "Search…",
  error,
  className,
  empty,
  keepOpen = false,
}: {
  label: string;
  items: T[];
  itemKey: (item: T) => string;
  /** Text the search matches. */
  itemText: (item: T) => string;
  renderItem?: (item: T) => ReactNode;
  selected: (item: T) => boolean;
  onSelect: (item: T) => void;
  /** Content of the button opening the list. */
  trigger: ReactNode;
  placeholder?: string;
  error?: string;
  className?: string;
  /** Shown when the search matches nothing. It gets the search. */
  empty?: (query: string, close: () => void) => ReactNode;
  /** Keep the list open after a choice, to pick several. */
  keepOpen?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const id = useId();
  const labelId = `${id}-label`;
  return (
    <Field label={label} error={error} labelId={labelId} className={className}>
      <Popover
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) setQuery("");
        }}
      >
        <PopoverTrigger
          id={id}
          aria-labelledby={`${labelId} ${id}`}
          aria-invalid={!!error}
          className="flex min-h-8 w-full cursor-pointer items-center justify-between gap-2 rounded-lg border border-input bg-card px-2.5 py-1 text-left text-sm outline-none transition-colors focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 aria-invalid:border-destructive"
        >
          <span className="min-w-0 grow">{trigger}</span>
          <ChevronsUpDown className="size-4 shrink-0 text-muted-foreground" />
        </PopoverTrigger>
        <PopoverContent align="start" className="w-(--anchor-width) min-w-56 p-0">
          <Command shouldFilter>
            <CommandInput
              placeholder={placeholder}
              value={query}
              onValueChange={setQuery}
            />
            <CommandList>
              <CommandEmpty className="px-2 py-3 text-sm">
                {empty ? empty(query, () => setOpen(false)) : "No results."}
              </CommandEmpty>
              <CommandGroup>
                {items.map((item) => (
                  <CommandItem
                    key={itemKey(item)}
                    value={itemText(item)}
                    onSelect={() => {
                      onSelect(item);
                      if (!keepOpen) setOpen(false);
                    }}
                  >
                    <span className="flex min-w-0 grow items-center gap-2">
                      {renderItem ? renderItem(item) : itemText(item)}
                    </span>
                    {selected(item) && <Check className="size-4 shrink-0" />}
                  </CommandItem>
                ))}
              </CommandGroup>
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
    </Field>
  );
}
