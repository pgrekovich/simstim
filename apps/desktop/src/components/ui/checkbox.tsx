import * as React from "react"
import { CheckIcon } from "lucide-react"
import { Checkbox as CheckboxPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

function Checkbox({
  className,
  variant = "checkbox",
  children,
  ...props
}: React.ComponentProps<typeof CheckboxPrimitive.Root> & {
  variant?: "checkbox" | "switch"
}) {
  if (variant === "switch") {
    return (
      <CheckboxPrimitive.Root
        data-slot="checkbox"
        className={cn(
          "peer flex h-[22px] w-[38px] shrink-0 items-center rounded-full bg-foreground/15 p-[3px] outline-none transition-colors",
          "hover:bg-foreground/20 data-[state=checked]:bg-primary",
          "focus-visible:ring-[3px] focus-visible:ring-ring/40",
          "disabled:cursor-not-allowed disabled:opacity-40",
          "[&[data-state=checked]>span]:translate-x-4",
          className
        )}
        {...props}
      >
        <span className="size-4 rounded-full bg-background shadow-[0_1px_2px_rgb(0_0_0/0.25)] transition-transform" />
        {children}
      </CheckboxPrimitive.Root>
    )
  }

  return (
    <CheckboxPrimitive.Root
      data-slot="checkbox"
      className={cn(
        "peer size-4 shrink-0 rounded-[4px] border border-input shadow-xs outline-none transition-shadow dark:bg-input/30",
        "data-[state=checked]:border-primary data-[state=checked]:bg-primary data-[state=checked]:text-primary-foreground dark:data-[state=checked]:bg-primary",
        "focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50",
        "disabled:cursor-not-allowed disabled:opacity-50",
        className
      )}
      {...props}
    >
      {children}
      <CheckboxPrimitive.Indicator
        data-slot="checkbox-indicator"
        className="flex items-center justify-center text-current transition-none"
      >
        <CheckIcon className="size-3.5" />
      </CheckboxPrimitive.Indicator>
    </CheckboxPrimitive.Root>
  )
}

export { Checkbox }
