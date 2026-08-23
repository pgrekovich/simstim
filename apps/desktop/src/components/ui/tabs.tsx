import * as React from "react"
import { Tabs as TabsPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

function Tabs({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.Root>) {
  return (
    <TabsPrimitive.Root
      data-slot="tabs"
      className={cn("flex flex-col", className)}
      {...props}
    />
  )
}

function TabsList({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.List>) {
  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      className={cn("inline-flex h-8 items-center gap-1", className)}
      {...props}
    />
  )
}

function TabsTrigger({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      data-slot="tabs-trigger"
      className={cn(
        // The active tab is a solid inverted pill: foreground fill,
        // background text. Every alpha-tint variant tried before this
        // (bg-background inside a tinted list, then the sidebar-row 9%
        // tint) read as mush on the identical bg-background content pane,
        // especially in the light theme. Inversion flips with the theme,
        // so it's unmissable in both.
        "inline-flex h-7 items-center justify-center rounded-md px-3 text-sm text-muted-foreground outline-none transition-colors duration-150",
        "hover:bg-foreground/[0.07] hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/50",
        "data-[state=active]:bg-foreground data-[state=active]:font-medium data-[state=active]:text-background",
        className
      )}
      {...props}
    />
  )
}

function TabsContent({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Content>) {
  return (
    <TabsPrimitive.Content
      data-slot="tabs-content"
      className={cn("outline-none", className)}
      {...props}
    />
  )
}

export { Tabs, TabsList, TabsTrigger, TabsContent }
