import { RefreshCw, ShieldCheck, Terminal } from "lucide-react";

import { Button } from "@/shared/ui/button";

export function ProjectViewV3SetupGuide({
  onRefresh,
  refreshing,
  relayPubkey,
}: {
  onRefresh: () => void;
  refreshing: boolean;
  relayPubkey: string;
}) {
  return (
    <main
      className="min-h-0 flex-1 overflow-y-auto p-5"
      data-testid="project-view-v3-setup-guide"
    >
      <div className="mx-auto max-w-3xl">
        <div className="flex items-start gap-3">
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-border/70 bg-muted/30">
            <ShieldCheck className="h-4 w-4 text-muted-foreground" />
          </div>
          <div>
            <h1 className="text-xl font-semibold">
              Project View v3 requires owner initialization
            </h1>
            <p className="mt-1 max-w-2xl text-sm leading-relaxed text-muted-foreground">
              Desktop did not find an initialized canonical Project View for
              this Community. Desktop cannot perform the privileged bootstrap.
            </p>
          </div>
        </div>

        <section className="mt-6 space-y-4 rounded-2xl border border-border/70 bg-card/60 p-5">
          <div className="flex gap-3">
            <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-semibold">
              1
            </span>
            <div className="min-w-0">
              <h2 className="text-sm font-semibold">
                Operator prepares the v3 bootstrap
              </h2>
              <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
                A Relay operator must prepare this Community for schema v3 and
                give the preparation receipt to its owner. The preparation is
                bound to the owner identity and cannot be replaced by a normal
                Desktop mutation.
              </p>
              <code className="mt-3 block overflow-x-auto rounded-lg border border-border/70 bg-background px-3 py-2 text-xs">
                buzz-admin project-view prepare-v3 --community
                &lt;community-host&gt; --idempotency-key &lt;key&gt;
                --operator-pubkey &lt;owner-pubkey&gt;
              </code>
            </div>
          </div>

          <div className="border-t border-border/70 pt-4">
            <div className="flex gap-3">
              <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-semibold">
                2
              </span>
              <div className="min-w-0">
                <h2 className="text-sm font-semibold">
                  Community owner initializes the canonical View
                </h2>
                <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
                  The owner reviews the complete bootstrap command, including
                  the Project Profile, initial Goal, governance Role, and
                  Assignment, then signs and submits it with the Carryforth CLI.
                </p>
                <code className="mt-3 block overflow-x-auto rounded-lg border border-border/70 bg-background px-3 py-2 text-xs">
                  cf project-view init-v3 --command
                  &lt;prepared-bootstrap.json&gt;
                </code>
              </div>
            </div>
          </div>
        </section>

        <div className="mt-4 rounded-xl border border-border/70 bg-muted/20 px-4 py-3">
          <div className="flex items-start gap-2">
            <Terminal className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
            <div className="min-w-0 text-xs leading-relaxed text-muted-foreground">
              <p>
                Relay identity: <code className="break-all">{relayPubkey}</code>
              </p>
              <p className="mt-1">
                After the owner command succeeds, check again to load the
                verified schema-v3 projection.
              </p>
            </div>
          </div>
        </div>

        <div className="mt-4 flex justify-end">
          <Button disabled={refreshing} onClick={onRefresh} type="button">
            <RefreshCw className={refreshing ? "animate-spin" : undefined} />
            {refreshing ? "Checking…" : "Check for initialized View"}
          </Button>
        </div>
      </div>
    </main>
  );
}
