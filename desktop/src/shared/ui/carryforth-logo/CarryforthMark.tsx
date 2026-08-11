import { cn } from "@/shared/lib/cn";

/** Carryforth's product mark: two linked forward strokes in a rounded frame. */
export function CarryforthMark({
  ariaLabel,
  className,
}: {
  ariaLabel?: string;
  className?: string;
}) {
  return (
    <svg
      aria-hidden={ariaLabel ? undefined : true}
      aria-label={ariaLabel}
      className={cn("block", className)}
      fill="none"
      viewBox="0 0 64 64"
      role={ariaLabel ? "img" : undefined}
      xmlns="http://www.w3.org/2000/svg"
    >
      <rect
        height="54"
        rx="16"
        stroke="currentColor"
        strokeWidth="4"
        width="54"
        x="5"
        y="5"
      />
      <path
        d="M18 20 30 32 18 44M32 20l12 12-12 12"
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="5"
      />
    </svg>
  );
}
