import { RecoveryScreen } from "./RecoveryScreen";

export function RelaunchRequiredScreen() {
  return (
    <RecoveryScreen
      testId="relaunch-required"
      title="Restart Carryforth to finish recovery"
      body="Your identity was updated. Carryforth needs to restart so syncing and agents run under it."
    />
  );
}
