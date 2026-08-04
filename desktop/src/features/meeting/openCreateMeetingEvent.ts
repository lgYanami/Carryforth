const OPEN_CREATE_MEETING_EVENT = "buzz:open-create-meeting";

export type OpenCreateMeetingOptions = {
  /** Optional ordinary Channel prefilled as a removable source reference. */
  sourceChannelId?: string;
};

export function requestOpenCreateMeeting(
  options: OpenCreateMeetingOptions = {},
) {
  window.dispatchEvent(
    new CustomEvent<OpenCreateMeetingOptions>(OPEN_CREATE_MEETING_EVENT, {
      detail: options,
    }),
  );
}

export function subscribeOpenCreateMeeting(
  handler: (options: OpenCreateMeetingOptions) => void,
) {
  function handleOpenCreateMeeting(event: Event) {
    handler(
      event instanceof CustomEvent
        ? (event.detail as OpenCreateMeetingOptions)
        : {},
    );
  }

  window.addEventListener(OPEN_CREATE_MEETING_EVENT, handleOpenCreateMeeting);
  return () => {
    window.removeEventListener(
      OPEN_CREATE_MEETING_EVENT,
      handleOpenCreateMeeting,
    );
  };
}
