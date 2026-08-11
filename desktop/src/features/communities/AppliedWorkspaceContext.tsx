import * as React from "react";

import type { AppliedWorkspaceIdentity } from "@/shared/api/tauri";

const AppliedWorkspaceContext = React.createContext<
  AppliedWorkspaceIdentity | undefined
>(undefined);

/** Supplies the atomically applied native workspace identity to this Community tree. */
export function AppliedWorkspaceProvider({
  children,
  value,
}: {
  children: React.ReactNode;
  value: AppliedWorkspaceIdentity;
}) {
  return (
    <AppliedWorkspaceContext.Provider value={value}>
      {children}
    </AppliedWorkspaceContext.Provider>
  );
}

/** Returns the native workspace identity captured for the current Community mount. */
export function useAppliedWorkspaceIdentity(): AppliedWorkspaceIdentity {
  const value = React.useContext(AppliedWorkspaceContext);
  if (!value) {
    throw new Error(
      "useAppliedWorkspaceIdentity must be used inside AppliedWorkspaceProvider",
    );
  }
  return value;
}
