import { CarryforthMark } from "@/shared/ui/carryforth-logo/CarryforthMark";

const MOTIFS = [
  { top: "8%", left: "8%", size: 30, rotate: -8 },
  { top: "11%", left: "75%", size: 24, rotate: 10 },
  { top: "27%", left: "18%", size: 20, rotate: 8 },
  { top: "34%", left: "88%", size: 32, rotate: -12 },
  { top: "59%", left: "7%", size: 28, rotate: 12 },
  { top: "68%", left: "78%", size: 22, rotate: -6 },
  { top: "84%", left: "24%", size: 24, rotate: -10 },
  { top: "88%", left: "90%", size: 30, rotate: 8 },
] as const;

/** Quiet first-run background composed from the Carryforth product mark. */
export function LandingBees() {
  return (
    <div
      aria-hidden
      className="pointer-events-none absolute inset-0 overflow-hidden"
    >
      <span className="absolute left-6 top-12 block w-11 text-foreground">
        <CarryforthMark className="h-auto w-full" />
      </span>
      {MOTIFS.map((motif) => (
        <span
          className="absolute block text-foreground/15"
          key={`${motif.top}-${motif.left}`}
          style={{
            top: motif.top,
            left: motif.left,
            width: motif.size,
            transform: `rotate(${motif.rotate}deg)`,
          }}
        >
          <CarryforthMark className="h-auto w-full" />
        </span>
      ))}
    </div>
  );
}
