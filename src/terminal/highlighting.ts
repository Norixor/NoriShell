export interface HighlightRule {
  id: string;
  label: string;
  pattern: string;
  mode: "literal" | "regex";
  caseSensitive: boolean;
  foreground: string;
  background: string;
  enabled: boolean;
}

export interface HighlightConfiguration {
  enabled: boolean;
  rules: HighlightRule[];
}

export interface HostHighlightConfiguration extends HighlightConfiguration {
  mode: "inherit" | "custom" | "disabled";
}

export interface HighlightMatch {
  line: number;
  start: number;
  end: number;
  ruleId: string;
}

export const MAX_HIGHLIGHT_RULES = 32;
export const MAX_HIGHLIGHT_PATTERN = 256;
export const MAX_HIGHLIGHT_MATCHES = 1_000;
export const DEFAULT_HIGHLIGHT_RULES: readonly HighlightRule[] = [
  { id: "error", label: "ERROR", pattern: "\\b(ERROR|FATAL|CRITICAL|PANIC|FAIL|FAILED|FAILURE)\\b", mode: "regex", caseSensitive: false, foreground: "#ffffff", background: "#a82d39", enabled: true },
  { id: "warning", label: "WARN", pattern: "\\b(WARN|WARNING)\\b", mode: "regex", caseSensitive: false, foreground: "#332200", background: "#f0c66c", enabled: true },
  { id: "success", label: "SUCCESS", pattern: "\\b(OK|SUCCESS|SUCCESSFUL|SUCCEEDED|PASS|PASSED)\\b", mode: "regex", caseSensitive: false, foreground: "#ffffff", background: "#237346", enabled: true },
  { id: "info", label: "INFO", pattern: "\\b(INFO|NOTICE)\\b", mode: "regex", caseSensitive: false, foreground: "#ffffff", background: "#285e9b", enabled: true },
  { id: "debug", label: "DEBUG / TRACE", pattern: "\\b(DEBUG|TRACE)\\b", mode: "regex", caseSensitive: false, foreground: "#ffffff", background: "#596273", enabled: false },
];

export function validateHighlightRule(value: unknown): value is HighlightRule {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const rule = value as Partial<HighlightRule>;
  if (
    typeof rule.id !== "string" || !/^[a-zA-Z0-9_-]{1,80}$/.test(rule.id)
    || typeof rule.label !== "string" || !rule.label.trim() || rule.label.length > 48
    || typeof rule.pattern !== "string" || !rule.pattern || rule.pattern.length > MAX_HIGHLIGHT_PATTERN
    // eslint-disable-next-line no-control-regex -- Explicitly reject invisible control characters in configured patterns.
    || /[\u0000-\u001f\u007f]/.test(rule.label + rule.pattern)
    || (rule.mode !== "literal" && rule.mode !== "regex")
    || typeof rule.caseSensitive !== "boolean" || typeof rule.enabled !== "boolean"
    || typeof rule.foreground !== "string" || !/^#[0-9a-fA-F]{6}$/.test(rule.foreground)
    || typeof rule.background !== "string" || !/^#[0-9a-fA-F]{6}$/.test(rule.background)
  ) return false;
  try {
    if (rule.mode === "regex") new RegExp(rule.pattern, "u");
    return true;
  } catch { return false; }
}

export function validateHighlightConfiguration(value: unknown): value is HighlightConfiguration {
  if (!value || typeof value !== "object") return false;
  const config = value as Partial<HighlightConfiguration>;
  return typeof config.enabled === "boolean"
    && Array.isArray(config.rules) && config.rules.length <= MAX_HIGHLIGHT_RULES
    && config.rules.every(validateHighlightRule)
    && new Set(config.rules.map((rule) => rule.id)).size === config.rules.length;
}

/** Run user regular expressions only in an isolated Worker; callers terminate runaway matching by wall-clock deadline. */
export function matchHighlightLines(lines: readonly string[], rules: readonly HighlightRule[]): HighlightMatch[] {
  const result: HighlightMatch[] = [];
  for (const rule of rules.slice(0, MAX_HIGHLIGHT_RULES)) {
    if (!rule.enabled || !validateHighlightRule(rule)) continue;
    const pattern = rule.mode === "literal" ? rule.pattern.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") : rule.pattern;
    const regex = new RegExp(pattern, rule.caseSensitive ? "gu" : "giu");
    for (const [line, input] of lines.slice(0, 250).entries()) {
      const text = input.slice(0, 4_096);
      regex.lastIndex = 0;
      for (const match of text.matchAll(regex)) {
        if (!match[0]) continue;
        result.push({ line, start: match.index, end: match.index + match[0].length, ruleId: rule.id });
        if (result.length >= MAX_HIGHLIGHT_MATCHES) return result;
      }
    }
  }
  return result;
}
