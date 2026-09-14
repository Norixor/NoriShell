import { openUrl } from "@tauri-apps/plugin-opener";

export const MAX_TERMINAL_URL_LENGTH = 4_096;

export interface TerminalHttpLink {
  /** Click-time snapshot after validation; never reread xterm callback arguments. */
  url: string;
  start: number;
  end: number;
}

// In addition to C0/C1, Unicode format controls such as bidi overrides can make a link's display differ from
// its opened target. Terminal links do not need these characters, so reject them all.
const controlCharacter = /\p{C}/u;
const encodedControlCharacter = /%(?:0[0-9a-f]|1[a-f]|7f|8[0-9a-f]|9[0-9a-f])/i;
const urlCandidate = /https?:\/\/[^\s<>"'`\\\p{Cc}]+/giu;

function urlAuthority(candidate: string) {
  const start = candidate.indexOf("://");
  if (start < 0) return "";
  const remainder = candidate.slice(start + 3);
  const end = remainder.search(/[/?#]/);
  return end < 0 ? remainder : remainder.slice(0, end);
}

function trimTerminalUrlCandidate(candidate: string) {
  let value = candidate;
  while (value.length) {
    if (/[,.;:!?]$/.test(value)) {
      value = value.slice(0, -1);
      continue;
    }
    const last = value.at(-1);
    if (last === ")" && (value.match(/\(/g)?.length ?? 0) < (value.match(/\)/g)?.length ?? 0)) {
      value = value.slice(0, -1);
      continue;
    }
    if (last === "]" && (value.match(/\[/g)?.length ?? 0) < (value.match(/\]/g)?.length ?? 0)) {
      value = value.slice(0, -1);
      continue;
    }
    if (last === "}" && (value.match(/\{/g)?.length ?? 0) < (value.match(/\}/g)?.length ?? 0)) {
      value = value.slice(0, -1);
      continue;
    }
    break;
  }
  return value;
}

/** Accept only userinfo-free HTTP(S) URLs that this app's controlled system opener can open. */
export function safeTerminalHttpUrl(value: string): string | null {
  const candidate = trimTerminalUrlCandidate(value);
  let decodedCandidate: string;
  try {
    // Also inspect percent-encoded control characters; undecodable URLs never reach the system opener.
    decodedCandidate = decodeURIComponent(candidate);
  } catch {
    return null;
  }
  if (!candidate
    || candidate.length > MAX_TERMINAL_URL_LENGTH
    || /\s/.test(candidate)
    || controlCharacter.test(candidate)
    || controlCharacter.test(decodedCandidate)
    || encodedControlCharacter.test(candidate)) return null;
  // URL normalizes empty userinfo to an empty string, so reject userinfo markers in the whole authority before parsing.
  const authority = urlAuthority(candidate);
  if (authority.includes("@") || /%40/i.test(authority)) return null;
  try {
    const parsed = new URL(candidate);
    if ((parsed.protocol !== "http:" && parsed.protocol !== "https:")
      || !parsed.hostname
      || parsed.username
      || parsed.password) return null;
    return candidate;
  } catch {
    return null;
  }
}

export function findTerminalHttpLinks(line: string): TerminalHttpLink[] {
  const links: TerminalHttpLink[] = [];
  urlCandidate.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = urlCandidate.exec(line)) !== null) {
    const url = safeTerminalHttpUrl(match[0]);
    if (!url) continue;
    links.push({ url, start: match.index, end: match.index + url.length });
  }
  return links;
}

export function terminalLinkModifierPressed(event: MouseEvent, platform: "macos" | "windows" | "other") {
  return platform === "macos" ? event.metaKey : event.ctrlKey;
}

/** Never use a browser fallback; always call Tauri's registered, capability-limited system opener. */
export async function openSafeTerminalHttpUrl(snapshot: string, opener: (url: string) => Promise<void> = openUrl) {
  const url = safeTerminalHttpUrl(snapshot);
  if (!url) return false;
  await opener(url);
  return true;
}
