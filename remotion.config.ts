import {Config} from "@remotion/cli/config";

export const REMOTION_ENTRY_POINT = "packages/remotion-composition/src/index.ts";
export const REMOTION_PUBLIC_DIRECTORY = "public";
export const REMOTION_BUNDLE_DIRECTORY = ".runtime/render-runtime/composition";

Config.setEntryPoint(REMOTION_ENTRY_POINT);
Config.setPublicDir(REMOTION_PUBLIC_DIRECTORY);
Config.setBundleOutDir(REMOTION_BUNDLE_DIRECTORY);
Config.setPublicPath("./");
Config.setCachingEnabled(false);
Config.setChromeMode("headless-shell");
