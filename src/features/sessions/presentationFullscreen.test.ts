// @vitest-environment jsdom

import {beforeEach, describe, expect, it, vi} from "vitest";

const tauri = vi.hoisted(() => ({
  enabled: false,
  isFullscreen: vi.fn(async () => false),
  setFullscreen: vi.fn(async () => undefined),
}));

vi.mock("@tauri-apps/api/core", () => ({isTauri: () => tauri.enabled}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    isFullscreen: tauri.isFullscreen,
    setFullscreen: tauri.setFullscreen,
  }),
}));

import {
  presentationFullscreenActive,
  setPresentationFullscreen,
  usesNativePresentationFullscreen,
} from "./presentationFullscreen";

describe("presentation fullscreen", () => {
  beforeEach(() => {
    tauri.enabled = false;
    tauri.isFullscreen.mockClear();
    tauri.setFullscreen.mockClear();
  });

  it("uses the browser fullscreen API outside Tauri", async () => {
    const target = document.createElement("section");
    const requestFullscreen = vi.fn(async () => undefined);
    Object.defineProperty(target, "requestFullscreen", {value: requestFullscreen});

    expect(usesNativePresentationFullscreen()).toBe(false);
    await setPresentationFullscreen(target, true);
    expect(requestFullscreen).toHaveBeenCalledOnce();
    expect(tauri.setFullscreen).not.toHaveBeenCalled();
  });

  it("uses the native desktop window in Tauri", async () => {
    tauri.enabled = true;
    tauri.isFullscreen.mockResolvedValueOnce(true);
    const target = document.createElement("section");

    expect(usesNativePresentationFullscreen()).toBe(true);
    expect(await presentationFullscreenActive(target)).toBe(true);
    await setPresentationFullscreen(target, false);
    expect(tauri.isFullscreen).toHaveBeenCalledOnce();
    expect(tauri.setFullscreen).toHaveBeenCalledWith(false);
  });

  it("rejects an unavailable browser fullscreen API instead of silently doing nothing", async () => {
    await expect(setPresentationFullscreen(document.body, true)).rejects.toThrow(
      "Fullscreen is unavailable",
    );
  });
});
