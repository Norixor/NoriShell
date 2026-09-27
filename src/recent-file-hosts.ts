const STORAGE_KEY = "norishell:recent-sftp-hosts";
export const RECENT_FILE_HOSTS_CHANGED = "norishell:recent-sftp-hosts-changed";

export function readRecentFileHosts(): Record<string, number> {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}");
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    return Object.fromEntries(Object.entries(value).filter(([hostId, timestamp]) =>
      typeof hostId === "string" && typeof timestamp === "number"
      && Number.isSafeInteger(timestamp) && timestamp > 0 && timestamp <= Date.now()));
  } catch {
    return {};
  }
}

export function recordRecentFileHost(hostId: string) {
  if (!hostId) return;
  try {
    const entries = Object.entries({ ...readRecentFileHosts(), [hostId]: Date.now() })
      .sort((a, b) => b[1] - a[1]).slice(0, 100);
    localStorage.setItem(STORAGE_KEY, JSON.stringify(Object.fromEntries(entries)));
    window.dispatchEvent(new Event(RECENT_FILE_HOSTS_CHANGED));
  } catch {
    // Recent UI metadata must not turn a successful SFTP connection into a failure.
  }
}
