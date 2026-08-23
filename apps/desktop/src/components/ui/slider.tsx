import * as React from "react"
import { Slider as SliderPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

function Slider({
  className,
  ...props
}: React.ComponentProps<typeof SliderPrimitive.Root>) {
  return (
    <SliderPrimitive.Root
      data-slot="slider"
      className={cn(
        "relative flex w-full touch-none select-none items-center",
        className
      )}
      {...props}
    >
      <SliderPrimitive.Track className="relative h-1 grow overflow-hidden rounded-full bg-foreground/[0.12]">
        <SliderPrimitive.Range className="absolute h-full bg-foreground/70" />
      </SliderPrimitive.Track>
      <SliderPrimitive.Thumb
        className="block size-3 rounded-full border border-foreground/30 bg-background shadow-sm outline-none transition-transform focus-visible:ring-[3px] focus-visible:ring-ring/50 active:scale-110"
        aria-label="Seek"
      />
    </SliderPrimitive.Root>
  )
}

export { Slider }
