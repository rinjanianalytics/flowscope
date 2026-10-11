// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

"use client";

import {
  DndContext,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  rectSortingStrategy,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { X } from "lucide-react";
import { Combobox } from "@/components/visualize/field";
import { useIsDark } from "@/components/providers";
import { dataColor } from "@/lib/palette";

const groups = ["Exporter", "Src", "Dst", "In", "Out", ""];

/** Color of a dimension, from the group of its name. */
const dimensionColor = (name: string, theme: "light" | "dark") =>
  dataColor(
    groups.findIndex((prefix) => name.startsWith(prefix)),
    false,
    theme,
  );

function Chip({
  name,
  color,
  onRemove,
}: {
  name: string;
  color: string;
  onRemove: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition } =
    useSortable({ id: name });
  return (
    <span
      ref={setNodeRef}
      style={{
        transform: CSS.Translate.toString(transform),
        transition,
        borderColor: color,
      }}
      className="flex cursor-grab items-center gap-1 rounded-full border-2 bg-accent py-0.5 pl-2 pr-1 text-xs font-medium text-accent-foreground"
      {...attributes}
      {...listeners}
    >
      <span className="leading-4">{name}</span>
      <span
        role="button"
        aria-label={`Remove ${name}`}
        className="-my-1 -mr-1 flex size-6 cursor-pointer items-center justify-center rounded-full hover:text-foreground"
        onPointerDown={(event) => event.stopPropagation()}
        onClick={(event) => {
          event.stopPropagation();
          event.preventDefault();
          onRemove();
        }}
      >
        <X className="size-3.5" />
      </span>
    </span>
  );
}

/** The dimensions to group on. They can be reordered by dragging them. */
export function DimensionsInput({
  available,
  selected,
  onChange,
  error,
  className,
}: {
  available: string[];
  selected: string[];
  onChange: (selected: string[]) => void;
  error?: string;
  className?: string;
}) {
  const theme = useIsDark() ? "dark" : "light";
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
  );
  const onDragEnd = ({ active, over }: DragEndEvent) => {
    if (!over || active.id === over.id) return;
    onChange(
      arrayMove(
        selected,
        selected.indexOf(String(active.id)),
        selected.indexOf(String(over.id)),
      ),
    );
  };
  const toggle = (name: string) =>
    onChange(
      selected.includes(name)
        ? selected.filter((d) => d !== name)
        : [...selected, name],
    );
  return (
    <Combobox
      label="Dimensions"
      items={available}
      itemKey={(name) => name}
      itemText={(name) => name}
      selected={(name) => selected.includes(name)}
      onSelect={toggle}
      keepOpen
      error={error}
      className={className}
      placeholder="Search dimensions…"
      renderItem={(name) => (
        <>
          <span
            className="h-4 w-1 rounded"
            style={{ backgroundColor: dimensionColor(name, theme) }}
          />
          {name}
        </>
      )}
      trigger={
        selected.length === 0 ? (
          <span className="text-muted-foreground">No dimensions</span>
        ) : (
          <DndContext
            sensors={sensors}
            collisionDetection={closestCenter}
            onDragEnd={onDragEnd}
          >
            <SortableContext items={selected} strategy={rectSortingStrategy}>
              <span className="flex flex-wrap gap-1 py-0.5">
                {selected.map((name) => (
                  <Chip
                    key={name}
                    name={name}
                    color={dimensionColor(name, theme)}
                    onRemove={() => toggle(name)}
                  />
                ))}
              </span>
            </SortableContext>
          </DndContext>
        )
      }
    />
  );
}
