import { cn } from "@/shared/lib/cn";
import type { CSSProperties } from "react";

const STARTER_BADGE_STYLES: Record<string, string> = {
  "builtin:fizz":
    "border-amber-300/70 bg-amber-100 text-amber-950 dark:border-amber-500/60 dark:bg-amber-950 dark:text-amber-100",
  "builtin:honey":
    "border-orange-300/70 bg-orange-100 text-orange-950 dark:border-orange-500/60 dark:bg-orange-950 dark:text-orange-100",
  "builtin:bumble":
    "border-sky-300/70 bg-sky-100 text-sky-950 dark:border-sky-500/60 dark:bg-sky-950 dark:text-sky-100",
};

const STARTER_BADGE_INITIALS: Record<string, string> = {
  "builtin:fizz": "F",
  "builtin:honey": "H",
  "builtin:bumble": "B",
};

/** A product-owned, code-rendered replacement for Starter Team artwork. */
export function StarterPersonaBadge({
  className,
  displayName,
  personaId,
  style,
  testId,
}: {
  className?: string;
  displayName: string;
  personaId: string;
  style?: CSSProperties;
  testId?: string;
}) {
  const displayInitial = displayName.trim().charAt(0).toUpperCase();
  const initial = STARTER_BADGE_INITIALS[personaId] ?? (displayInitial || "?");

  return (
    <span
      aria-label={`${displayName} starter persona`}
      className={cn(
        "relative inline-flex aspect-square shrink-0 items-center justify-center overflow-hidden rounded-[28%] border shadow-sm",
        STARTER_BADGE_STYLES[personaId] ??
          "border-border bg-muted text-foreground",
        className,
      )}
      data-persona-id={personaId}
      data-testid={testId}
      role="img"
      style={style}
    >
      <span
        aria-hidden="true"
        className="absolute inset-[12%] rounded-[24%] border border-current/20"
      />
      <span
        aria-hidden="true"
        className="absolute right-[14%] top-[14%] h-[18%] w-[18%] rounded-full bg-current/15"
      />
      <span className="relative font-mono text-3xl font-semibold leading-none">
        {initial}
      </span>
    </span>
  );
}
