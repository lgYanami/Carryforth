export type PersonaModelDiscoveryStatus = {
  message: string;
  tone: "muted" | "warning";
};

function errorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string") {
    return error;
  }
  try {
    return JSON.stringify(error);
  } catch {
    return "Unknown model discovery error";
  }
}

function providerObjectLabel(provider: string): string {
  switch (provider.trim()) {
    case "anthropic":
      return "Anthropic";
    case "openai":
      return "OpenAI";
    case "openai-compat":
      return "OpenAI-compatible";
    default:
      return provider.trim() || "this provider";
  }
}

function isEmptySharedComputeError(message: string): boolean {
  const normalized = message.toLowerCase();
  return (
    normalized.includes("shared compute status is not published") ||
    normalized.includes("no buzz shared compute serving members") ||
    normalized.includes("no carryforth shared compute serving members") ||
    normalized.includes("no live buzz shared compute models") ||
    normalized.includes("no live carryforth shared compute models") ||
    normalized.includes("no live member is serving") ||
    normalized.includes("requires a live serving member")
  );
}

export function formatModelDiscoveryErrorStatus(
  error: unknown,
  provider: string,
): PersonaModelDiscoveryStatus | null {
  const message = errorMessage(error);

  if (provider.trim() === "relay-mesh") {
    if (
      message.toLowerCase().includes("waiting for the current member roster")
    ) {
      return {
        message:
          "Carryforth is waiting for the Relay's member roster. Try again shortly; if this persists, check the Relay's membership configuration.",
        tone: "warning",
      };
    }

    if (isEmptySharedComputeError(message)) {
      return {
        message:
          "No members are sharing compute right now. On a member machine, open Settings > Compute, choose a model, and turn on Share this machine.",
        tone: "warning",
      };
    }

    if (message.includes("shared compute is not available in this build")) {
      return {
        message:
          "This version of Carryforth cannot use shared compute. Choose another provider.",
        tone: "warning",
      };
    }

    if (message.includes("shared compute status is malformed")) {
      return {
        message:
          "Carryforth received an invalid shared compute status. Check the member machine, then try again.",
        tone: "warning",
      };
    }

    return {
      message:
        "Carryforth couldn't check shared compute through the Relay. Check your Relay connection and try again.",
      tone: "warning",
    };
  }

  if (message.includes("ANTHROPIC_API_KEY required")) {
    return {
      message: "Enter an Anthropic API key to load Anthropic models.",
      tone: "warning",
    };
  }

  if (message.includes("OPENAI_COMPAT_API_KEY required")) {
    return {
      message: "Enter an OpenAI API key to load OpenAI models.",
      tone: "warning",
    };
  }

  if (
    message.includes("DATABRICKS_HOST required") ||
    message.includes("DATABRICKS_MODEL required") ||
    message.includes("BUZZ_AGENT_PROVIDER is required")
  ) {
    return null;
  }

  return {
    message: `Using built-in model options. Could not load live models for ${providerObjectLabel(
      provider,
    )}.`,
    tone: "warning",
  };
}
