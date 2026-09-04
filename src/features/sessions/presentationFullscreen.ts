import {isTauri} from "@tauri-apps/api/core";
import {getCurrentWindow} from "@tauri-apps/api/window";

export function usesNativePresentationFullscreen(): boolean {
  return isTauri();
}

export async function presentationFullscreenActive(target: HTMLElement): Promise<boolean> {
  return isTauri()
    ? getCurrentWindow().isFullscreen()
    : document.fullscreenElement === target;
}

export async function setPresentationFullscreen(
  target: HTMLElement,
  fullscreen: boolean,
): Promise<void> {
  if (isTauri()) {
    await getCurrentWindow().setFullscreen(fullscreen);
    return;
  }
  if (fullscreen) {
    if (typeof target.requestFullscreen !== "function") {
      throw new Error("Fullscreen is unavailable");
    }
    await target.requestFullscreen();
  } else if (document.fullscreenElement !== null) {
    await document.exitFullscreen();
  }
}
