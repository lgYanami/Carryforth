import { motion, useReducedMotion } from "motion/react";

import { cn } from "@/shared/lib/cn";
import { CarryforthMark } from "@/shared/ui/carryforth-logo/CarryforthMark";
import { useTranscriptAnimationEnabled } from "./transcriptAnimationPreference";

const MARKS = ["first", "second", "third"] as const;
const STAGGER_SECONDS = 0.25;
const CYCLE_SECONDS = 1.8;

export function TurnLivenessIndicator({
  className,
}: {
  className?: string;
  /** Defaults to false — the indicator stays mounted for whole turns. */
  fuzz?: boolean;
}) {
  const animationsEnabled = useTranscriptAnimationEnabled();
  const shouldReduceMotion = useReducedMotion();
  const showStaggeredRow = animationsEnabled && !shouldReduceMotion;

  if (!showStaggeredRow) {
    return (
      <div
        aria-label="Agent turn in progress"
        className={cn("opacity-25", className)}
        data-testid="turn-liveness-indicator"
        role="status"
      >
        <CarryforthMark
          ariaLabel="Agent turn in progress"
          className="h-6 w-6 text-foreground"
        />
      </div>
    );
  }

  return (
    <div
      aria-label="Agent turn in progress"
      className={cn("flex items-center gap-1.5 opacity-25", className)}
      data-testid="turn-liveness-indicator"
      role="status"
    >
      {MARKS.map((mark, index) => (
        <motion.div
          animate={{
            opacity: [0, 1, 1, 0],
            y: [4, 0, -1, -4],
          }}
          key={mark}
          transition={{
            delay: index * STAGGER_SECONDS,
            duration: CYCLE_SECONDS,
            ease: "easeInOut",
            repeat: Number.POSITIVE_INFINITY,
            times: [0, 0.3, 0.7, 1],
          }}
        >
          <CarryforthMark className="h-5 w-5 text-foreground" />
        </motion.div>
      ))}
    </div>
  );
}
