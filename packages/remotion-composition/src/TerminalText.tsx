import type {CSSProperties, ReactNode} from "react";
import type {ThemeSpec} from "@ai-session-replay/replay-contract";

export interface TerminalTextSegment {
  readonly text: string;
  readonly style: CSSProperties;
}

interface SgrState {
  foreground: string | null;
  background: string | null;
  bold: boolean;
  dim: boolean;
  italic: boolean;
  underline: boolean;
  inverse: boolean;
  concealed: boolean;
  strike: boolean;
}

const ESC = "\u001b";
const CSI = "\u009b";
const OSC = "\u009d";
const ST = "\u009c";

export function TerminalText({text, theme}: Readonly<{text: string; theme: ThemeSpec}>) {
  if (!containsTerminalControls(text)) return text;
  return parseTerminalText(text, theme).map((segment, index): ReactNode =>
    Object.keys(segment.style).length === 0
      ? segment.text
      : <span data-terminal-style="true" key={index} style={segment.style}>{segment.text}</span>,
  );
}

function containsTerminalControls(text: string): boolean {
  return /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/.test(text);
}

export function parseTerminalText(text: string, theme: ThemeSpec): readonly TerminalTextSegment[] {
  const segments: TerminalTextSegment[] = [];
  const state = initialState();
  let printable = "";
  let index = 0;
  const flush = () => {
    if (printable.length === 0) return;
    const style = terminalStyle(state, theme);
    const previous = segments.at(-1);
    if (previous && sameStyle(previous.style, style)) {
      segments[segments.length - 1] = {...previous, text: previous.text + printable};
    } else {
      segments.push({text: printable, style});
    }
    printable = "";
  };

  while (index < text.length) {
    const character = text[index]!;
    if (character === ESC || character === CSI) {
      flush();
      const csiStart = character === CSI ? index + 1 : text[index + 1] === "[" ? index + 2 : -1;
      if (csiStart >= 0) {
        const end = controlSequenceEnd(text, csiStart);
        if (end < 0) break;
        if (text[end] === "m") applySgr(state, text.slice(csiStart, end), theme);
        index = end + 1;
        continue;
      }
      if (character === ESC && text[index + 1] === "]") {
        index = stringControlEnd(text, index + 2);
        continue;
      }
      if (character === ESC && ["P", "X", "^", "_"].includes(text[index + 1] ?? "")) {
        index = stringControlEnd(text, index + 2);
        continue;
      }
      index += character === ESC && index + 1 < text.length ? 2 : 1;
      continue;
    }
    if (character === OSC || ["\u0090", "\u0098", "\u009e", "\u009f"].includes(character)) {
      flush();
      index = stringControlEnd(text, index + 1);
      continue;
    }
    const code = character.charCodeAt(0);
    if (character === "\r") {
      printable += "\n";
      if (text[index + 1] === "\n") index += 1;
    } else if (character === "\b") {
      printable = printable.slice(0, -1);
    } else if (character === "\n" || character === "\t" || code >= 0x20) {
      if (code < 0x7f || code > 0x9f) printable += character;
    }
    index += 1;
  }
  flush();
  return segments;
}

function initialState(): SgrState {
  return {
    foreground: null,
    background: null,
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    inverse: false,
    concealed: false,
    strike: false,
  };
}

function resetState(state: SgrState): void {
  Object.assign(state, initialState());
}

function applySgr(state: SgrState, rawParameters: string, theme: ThemeSpec): void {
  const parameters = rawParameters === ""
    ? [0]
    : rawParameters.split(";").map((value) => Number(value === "" ? 0 : value));
  for (let index = 0; index < parameters.length; index += 1) {
    const code = parameters[index];
    if (typeof code !== "number" || !Number.isInteger(code)) continue;
    if (code === 0) resetState(state);
    else if (code === 1) state.bold = true;
    else if (code === 2) state.dim = true;
    else if (code === 3) state.italic = true;
    else if (code === 4 || code === 21) state.underline = true;
    else if (code === 7) state.inverse = true;
    else if (code === 8) state.concealed = true;
    else if (code === 9) state.strike = true;
    else if (code === 22) { state.bold = false; state.dim = false; }
    else if (code === 23) state.italic = false;
    else if (code === 24) state.underline = false;
    else if (code === 27) state.inverse = false;
    else if (code === 28) state.concealed = false;
    else if (code === 29) state.strike = false;
    else if (code === 39) state.foreground = null;
    else if (code === 49) state.background = null;
    else if (code >= 30 && code <= 37) state.foreground = ansiColor(code - 30, false, theme);
    else if (code >= 90 && code <= 97) state.foreground = ansiColor(code - 90, true, theme);
    else if (code >= 40 && code <= 47) state.background = ansiColor(code - 40, false, theme);
    else if (code >= 100 && code <= 107) state.background = ansiColor(code - 100, true, theme);
    else if (code === 38 || code === 48) {
      const color = extendedColor(parameters, index + 1);
      if (color) {
        if (code === 38) state.foreground = color.value;
        else state.background = color.value;
        index += color.consumed;
      }
    }
  }
}

function extendedColor(
  parameters: readonly number[],
  start: number,
): Readonly<{value: string; consumed: number}> | null {
  if (parameters[start] === 5 && validByte(parameters[start + 1])) {
    return {value: indexedColor(parameters[start + 1]!), consumed: 2};
  }
  if (
    parameters[start] === 2 &&
    validByte(parameters[start + 1]) &&
    validByte(parameters[start + 2]) &&
    validByte(parameters[start + 3])
  ) {
    return {
      value: `rgb(${parameters[start + 1]}, ${parameters[start + 2]}, ${parameters[start + 3]})`,
      consumed: 4,
    };
  }
  return null;
}

function validByte(value: number | undefined): value is number {
  return Number.isInteger(value) && value !== undefined && value >= 0 && value <= 255;
}

function ansiColor(index: number, bright: boolean, theme: ThemeSpec): string {
  const regular = [theme.background, theme.error, theme.success, theme.accent, "#7AA2F7", "#BB9AF7", "#7DCFFF", theme.text];
  const intense = [theme.muted, theme.error, theme.success, theme.accent, "#9AB8FF", "#D2ADFF", "#A5E5FF", "#FFFFFF"];
  return (bright ? intense : regular)[index] ?? theme.text;
}

function indexedColor(index: number): string {
  if (index < 16) {
    const palette = [
      "#000000", "#800000", "#008000", "#808000", "#000080", "#800080", "#008080", "#C0C0C0",
      "#808080", "#FF0000", "#00FF00", "#FFFF00", "#0000FF", "#FF00FF", "#00FFFF", "#FFFFFF",
    ];
    return palette[index]!;
  }
  if (index < 232) {
    const value = index - 16;
    const channel = (offset: number) => {
      const component = Math.floor(value / (6 ** offset)) % 6;
      return component === 0 ? 0 : 55 + component * 40;
    };
    return `rgb(${channel(2)}, ${channel(1)}, ${channel(0)})`;
  }
  const gray = 8 + (index - 232) * 10;
  return `rgb(${gray}, ${gray}, ${gray})`;
}

function terminalStyle(state: SgrState, theme: ThemeSpec): CSSProperties {
  let color = state.foreground;
  let backgroundColor = state.background;
  if (state.inverse) {
    const previousColor = color ?? theme.text;
    color = backgroundColor ?? theme.background;
    backgroundColor = previousColor;
  }
  const decorations = [state.underline ? "underline" : null, state.strike ? "line-through" : null]
    .filter(Boolean)
    .join(" ");
  return {
    ...(color ? {color} : {}),
    ...(backgroundColor ? {backgroundColor} : {}),
    ...(state.bold ? {fontWeight: 700} : {}),
    ...(state.dim ? {opacity: 0.72} : {}),
    ...(state.italic ? {fontStyle: "italic"} : {}),
    ...(decorations ? {textDecoration: decorations} : {}),
    ...(state.concealed ? {visibility: "hidden"} : {}),
  };
}

function controlSequenceEnd(text: string, start: number): number {
  for (let index = start; index < text.length; index += 1) {
    const code = text.charCodeAt(index);
    if (code >= 0x40 && code <= 0x7e) return index;
  }
  return -1;
}

function stringControlEnd(text: string, start: number): number {
  for (let index = start; index < text.length; index += 1) {
    if (text[index] === "\u0007" || text[index] === ST) return index + 1;
    if (text[index] === ESC && text[index + 1] === "\\") return index + 2;
  }
  return text.length;
}

function sameStyle(left: CSSProperties, right: CSSProperties): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}
