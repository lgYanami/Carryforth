import * as React from "react";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { subscribeOpenCreateMeeting } from "@/features/meeting/openCreateMeetingEvent";

import { CreateMeetingDialog } from "./CreateMeetingDialog";

/** AppShell-owned overlay controller shared by every Meeting create entry. */
export function CreateMeetingDialogController() {
  const { goChannel } = useAppNavigation();
  const [open, setOpen] = React.useState(false);
  const [sourceChannelId, setSourceChannelId] = React.useState<string | null>(
    null,
  );
  const [requestVersion, setRequestVersion] = React.useState(0);

  React.useEffect(
    () =>
      subscribeOpenCreateMeeting((options) => {
        setSourceChannelId(options.sourceChannelId ?? null);
        setRequestVersion((current) => current + 1);
        setOpen(true);
      }),
    [],
  );

  return (
    <CreateMeetingDialog
      initialSourceChannelId={sourceChannelId}
      onCreated={(meetingId) => void goChannel(meetingId)}
      onOpenChange={setOpen}
      open={open}
      requestVersion={requestVersion}
    />
  );
}
