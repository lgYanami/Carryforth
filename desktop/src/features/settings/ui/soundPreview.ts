/** Pause the active notification preview when one exists. */
export function pauseSoundPreview(audio: HTMLAudioElement | null): void {
  audio?.pause();
}

/** End picker playback state when the audio pauses or reaches its end. */
export function listenForSoundPreviewStop(
  audio: HTMLAudioElement,
  onStop: () => void,
): void {
  audio.addEventListener("ended", onStop, { once: true });
  audio.addEventListener("pause", onStop, { once: true });
}
