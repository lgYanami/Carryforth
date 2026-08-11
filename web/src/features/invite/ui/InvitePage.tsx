import carryforthAppIcon from "@/assets/carryforth.svg";
import { claimInviteInBrowser } from "@/features/invite/invite-api";
import { CARRYFORTH_SOURCE_BUILD_URL } from "@/shared/lib/carryforth-source";
import { hasNip07Provider } from "@/shared/lib/nostr-signer";
import { relayWsUrl } from "@/shared/lib/relay-url";
import { Button } from "@/shared/ui/button";
import * as React from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";

import { InviteJoinPolicyNotice } from "./InviteJoinPolicyNotice";

type JoinPolicy = {
  terms_markdown?: string;
  privacy_markdown?: string;
  age_attestation_required: boolean;
  version: string;
};

type PolicyDocument = { title: string; markdown: string };

/** Landing page for a community invite link (`/invite/<code>`). */
export function InvitePage({ code }: { code: string }) {
  const relay = relayWsUrl();
  const host = relay.replace(/^wss?:\/\//, "");
  const [policy, setPolicy] = React.useState<JoinPolicy | null | undefined>(
    undefined,
  );
  const [document, setDocument] = React.useState<PolicyDocument | null>(null);
  const [ageConfirmed, setAgeConfirmed] = React.useState(false);
  const [agreementConfirmed, setAgreementConfirmed] = React.useState(false);
  const [joiningBrowser, setJoiningBrowser] = React.useState(false);
  const [browserJoinError, setBrowserJoinError] = React.useState<string | null>(
    null,
  );
  React.useEffect(() => {
    fetch("/api/join-policy")
      .then(async (response) => {
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        const config = (await response.json()) as { policy?: JoinPolicy };
        setPolicy(config.policy ?? null);
      })
      .catch(() => setPolicy(undefined));
  }, []);

  const acceptPolicy = async (): Promise<string | undefined> => {
    if (!policy) return undefined;
    const response = await fetch("/api/invites/accept-policy", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        code,
        policy_version: policy.version,
        age_confirmed: ageConfirmed,
      }),
    });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return ((await response.json()) as { receipt: string }).receipt;
  };

  const joinInBrowser = async () => {
    setBrowserJoinError(null);
    setJoiningBrowser(true);
    try {
      const receipt = await acceptPolicy();
      await claimInviteInBrowser(code, receipt);
      window.location.assign("/");
    } catch (error) {
      setBrowserJoinError(
        error instanceof Error ? error.message : "Could not claim this invite.",
      );
    } finally {
      setJoiningBrowser(false);
    }
  };

  const browserSigningAvailable = hasNip07Provider();
  const disabled =
    policy === undefined ||
    joiningBrowser ||
    Boolean(policy?.age_attestation_required && !ageConfirmed) ||
    Boolean(
      policy &&
        (policy.terms_markdown || policy.privacy_markdown) &&
        !agreementConfirmed,
    );
  const hasPolicyRequirements = Boolean(
    policy &&
      (policy.age_attestation_required ||
        policy.terms_markdown ||
        policy.privacy_markdown),
  );
  const showDocument = (title: string, markdown: string) =>
    setDocument({ title, markdown });

  return (
    <div
      className="flex flex-1 flex-col items-center justify-center px-4 py-16 text-center"
      style={{
        backgroundImage: "linear-gradient(180deg, #D7D72E 0%, #D7E7F6 100%)",
      }}
    >
      <div className="w-full max-w-xl space-y-4">
        <div className="flex w-full flex-col items-center rounded-3xl bg-white px-6 py-10 sm:px-12 sm:py-12">
          <div
            className="h-12 w-12 overflow-hidden bg-black"
            style={{ borderRadius: "22.37%" }}
          >
            <img
              alt="Carryforth"
              className="h-full w-full"
              src={carryforthAppIcon}
            />
          </div>
          <h1 className="mt-4 text-2xl font-semibold tracking-tight text-black">
            You&apos;re invited to
          </h1>
          <p className="mt-9 font-mono text-lg text-black/70">{host}</p>

          <div
            className={`grid w-full max-w-md overflow-hidden transition-[grid-template-rows,margin,opacity,transform] duration-[220ms] [transition-timing-function:cubic-bezier(0.23,1,0.32,1)] motion-reduce:transition-none ${
              hasPolicyRequirements
                ? "mt-9 -mb-4 grid-rows-[1fr] opacity-100 translate-y-0"
                : "m-0 grid-rows-[0fr] opacity-0 -translate-y-1"
            }`}
          >
            <div className="min-h-0 overflow-hidden">
              {policy && hasPolicyRequirements ? (
                <InviteJoinPolicyNotice
                  ageConfirmed={ageConfirmed}
                  agreementConfirmed={agreementConfirmed}
                  onAgeConfirmedChange={setAgeConfirmed}
                  onAgreementConfirmedChange={setAgreementConfirmed}
                  onShowDocument={showDocument}
                  policy={policy}
                />
              ) : null}
            </div>
          </div>

          <div className="mt-9 w-full max-w-md space-y-2">
            {browserSigningAvailable ? (
              <Button
                className="h-10 w-full bg-black text-white hover:bg-black/90 focus-visible:ring-black disabled:cursor-not-allowed disabled:bg-black/30 disabled:text-white/70"
                disabled={disabled}
                onClick={joinInBrowser}
              >
                {joiningBrowser ? "Joining…" : "Join in browser"}
              </Button>
            ) : null}
            {!browserSigningAvailable ? (
              <p
                className="rounded-xl border border-black/10 bg-black/[0.03] p-3 text-sm leading-5 text-black/60"
                data-testid="desktop-invite-handoff-unavailable"
              >
                Desktop invite handoff is not available from this source-only
                web client. Install a NIP-07 browser signer to join here.
              </p>
            ) : null}
            {browserJoinError ? (
              <p className="text-sm text-red-700" role="alert">
                {browserJoinError}
              </p>
            ) : null}
          </div>
        </div>
        <p className="flex h-[3.125rem] items-center justify-center rounded-2xl bg-white text-sm text-black/60">
          Need Carryforth Desktop?{" "}
          <a
            className="ml-1 font-medium text-black underline-offset-4 hover:text-black/70 hover:underline focus-visible:underline"
            href={CARRYFORTH_SOURCE_BUILD_URL}
            rel="noopener noreferrer"
            target="_blank"
          >
            Build it from source
          </a>
        </p>
      </div>

      {document && (
        <div
          aria-label={document.title}
          aria-modal="true"
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4 text-left"
          role="dialog"
          onMouseDown={(event) => {
            if (event.currentTarget === event.target) setDocument(null);
          }}
        >
          <div className="max-h-[85vh] w-full max-w-3xl overflow-y-auto rounded-2xl bg-white p-6 text-black shadow-xl sm:p-8">
            <div className="mb-6 flex items-start justify-between gap-4">
              <h2 className="text-xl font-semibold">{document.title}</h2>
              <button
                aria-label="Close"
                className="text-2xl leading-none text-black/60 hover:text-black"
                type="button"
                onClick={() => setDocument(null)}
              >
                ×
              </button>
            </div>
            <div className="prose prose-sm max-w-none">
              <Markdown remarkPlugins={[remarkGfm]}>
                {document.markdown}
              </Markdown>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
