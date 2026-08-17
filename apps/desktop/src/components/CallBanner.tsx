import { X } from "lucide-react";
import { Button } from "@/components/ui/button";

interface CallBannerProps {
  message: string;
  actionLabel: string;
  onAction: () => void;
  onDismiss: () => void;
  disabled?: boolean;
}

export function CallBanner({
  message,
  actionLabel,
  onAction,
  onDismiss,
  disabled = false,
}: CallBannerProps) {
  return (
    <div
      role="status"
      className="flex min-h-10 shrink-0 items-center gap-3 border-b bg-muted/60 px-4"
    >
      <span className="min-w-0 flex-1 truncate text-sm">{message}</span>
      <Button size="sm" disabled={disabled} onClick={onAction}>
        {actionLabel}
      </Button>
      <Button variant="ghost" size="icon-sm" aria-label="Dismiss" onClick={onDismiss}>
        <X />
      </Button>
    </div>
  );
}
