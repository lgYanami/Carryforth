import { cn } from "@/shared/lib/cn";
import { CarryforthMark } from "@/shared/ui/carryforth-logo/CarryforthMark";

const BUILT_IN_RUNTIME_ID = "buzz-agent";

/**
 * Product-owned runtime artwork shared by onboarding and settings.
 *
 * The bundled runtime uses the Carryforth mark. External runtimes use one
 * neutral terminal glyph so their factual names remain visible without
 * bundling or requesting provider logos.
 */
export function RuntimeGlyph({
  className,
  runtimeId,
  testId,
}: {
  className?: string;
  runtimeId: string;
  testId?: string;
}) {
  const isBuiltIn = runtimeId.trim().toLowerCase() === BUILT_IN_RUNTIME_ID;

  return (
    <span
      aria-hidden="true"
      className={cn(
        "inline-flex shrink-0 items-center justify-center text-foreground",
        className,
      )}
      data-testid={testId}
    >
      {isBuiltIn ? (
        <CarryforthMark className="h-full w-full" />
      ) : (
        <span className="relative h-full w-full rounded-[22%] border-[0.12em] border-current">
          <span className="absolute left-[20%] top-[36%] h-[20%] w-[20%] rotate-45 border-r-[0.1em] border-t-[0.1em] border-current" />
          <span className="absolute bottom-[25%] right-[18%] h-[0.1em] w-[32%] rounded-full bg-current" />
        </span>
      )}
    </span>
  );
}
