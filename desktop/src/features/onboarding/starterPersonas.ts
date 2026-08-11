import type { AgentPersona } from "@/shared/api/types";

export const STARTER_PERSONAS = [
  { id: "builtin:fizz", displayName: "Fizz" },
  { id: "builtin:honey", displayName: "Honey" },
  { id: "builtin:bumble", displayName: "Bumble" },
] as const;

/** Select and order Starter Team records by immutable persona identity. */
export function selectStarterPersonas(
  personas: readonly AgentPersona[],
): AgentPersona[] {
  return STARTER_PERSONAS.flatMap(({ id }) => {
    const persona = personas.find((candidate) => candidate.id === id);
    return persona ? [persona] : [];
  });
}
