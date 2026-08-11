import { ExternalLink } from "lucide-react";

import { CARRYFORTH_SOURCE_BUILD_URL } from "@/shared/lib/carryforth-source";
import { Button } from "@/shared/ui/button";

export function SourceBuildButton({ className }: { className?: string }) {
  return (
    <Button
      asChild
      className={`bg-black text-white hover:bg-black/90 focus-visible:ring-black dark:bg-white dark:text-black dark:hover:bg-white/90 dark:focus-visible:ring-white ${className ?? ""}`}
    >
      <a
        href={CARRYFORTH_SOURCE_BUILD_URL}
        rel="noopener noreferrer"
        target="_blank"
      >
        <ExternalLink className="h-4 w-4" />
        Build Carryforth from source
      </a>
    </Button>
  );
}
