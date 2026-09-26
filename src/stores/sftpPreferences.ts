import { defineStore } from "pinia";
import { ref } from "vue";
import { corePreferencesEnabled, saveApplicationPreferences } from "../core-api/application-preferences";

export const SFTP_PREFERENCES_KEY = "norishell.sftp.preferences.v1";
export interface SftpBrowserPreferences {
  showHidden: boolean;
  foldersFirst: boolean;
  sort: "name" | "size" | "modified";
}
export const DEFAULT_SFTP_PREFERENCES: Readonly<SftpBrowserPreferences> = {
  showHidden: true,
  foldersFirst: true,
  sort: "name",
};

const maximumRememberedHostDirectories = 100;
const maximumRememberedDirectoryBytes = 4_096;
const maximumRememberedHostIdBytes = 256;
const encoder = new TextEncoder();

interface RememberedRemoteDirectory {
  hostId: string;
  pathBytes: number[];
}

interface SftpDirectoryMemory {
  local: string | null;
  remote: RememberedRemoteDirectory[];
}

interface StoredSftpPreferences {
  browser: SftpBrowserPreferences;
  rememberLastDirectory: boolean;
  directories: SftpDirectoryMemory;
}

export function validSftpBrowserPreferences(value: unknown): value is SftpBrowserPreferences {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const fields = value as Record<string, unknown>;
  return Object.keys(fields).every((key) => ["showHidden", "foldersFirst", "sort"].includes(key))
    && typeof fields.showHidden === "boolean" && typeof fields.foldersFirst === "boolean"
    && ["name", "size", "modified"].includes(fields.sort as string);
}

function validPathBytes(value: unknown): value is number[] {
  return Array.isArray(value)
    && value.length > 0
    && value.length <= maximumRememberedDirectoryBytes
    && value.every((byte) => Number.isInteger(byte) && byte >= 0 && byte <= 255);
}

function validLocalDirectory(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && !value.includes("\0")
    && encoder.encode(value).byteLength <= maximumRememberedDirectoryBytes;
}

function validRememberedRemoteDirectory(value: unknown): value is RememberedRemoteDirectory {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const fields = value as Record<string, unknown>;
  return Object.keys(fields).every((key) => key === "hostId" || key === "pathBytes")
    && typeof fields.hostId === "string"
    && fields.hostId.length > 0
    && encoder.encode(fields.hostId).byteLength <= maximumRememberedHostIdBytes
    && validPathBytes(fields.pathBytes);
}

function validDirectoryMemory(value: unknown): value is SftpDirectoryMemory {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const fields = value as Record<string, unknown>;
  if (!Object.keys(fields).every((key) => key === "local" || key === "remote")
    || !Object.hasOwn(fields, "local") || !Object.hasOwn(fields, "remote")
    || !(fields.local === null || validLocalDirectory(fields.local))
    || !Array.isArray(fields.remote) || fields.remote.length > maximumRememberedHostDirectories
    || !fields.remote.every(validRememberedRemoteDirectory)) return false;
  return new Set(fields.remote.map((entry) => entry.hostId)).size === fields.remote.length;
}

function emptyDirectoryMemory(): SftpDirectoryMemory {
  return { local: null, remote: [] };
}

function copyDirectoryMemory(value: SftpDirectoryMemory): SftpDirectoryMemory {
  return {
    local: value.local,
    remote: value.remote.map((entry) => ({ hostId: entry.hostId, pathBytes: [...entry.pathBytes] })),
  };
}

function load(): StoredSftpPreferences {
  try {
    const raw = localStorage.getItem(SFTP_PREFERENCES_KEY);
    if (raw && raw.length <= 2 * 1024 * 1024) {
      const stored = JSON.parse(raw) as Record<string, unknown>;
      if (stored?.version === 1 && validSftpBrowserPreferences(stored.browser)) {
        // Early v1 stored only browser preferences; directory memory is always an explicitly enabled new field.
        const hasRememberSetting = Object.hasOwn(stored, "rememberLastDirectory");
        const storedDirectories = validDirectoryMemory(stored.directories) ? stored.directories : null;
        if (Object.keys(stored).every((key) => ["version", "browser", "rememberLastDirectory", "directories"].includes(key))
          && (!Object.hasOwn(stored, "rememberLastDirectory") || typeof stored.rememberLastDirectory === "boolean")
          && (!Object.hasOwn(stored, "directories") || storedDirectories)
          && (!Object.hasOwn(stored, "directories") || hasRememberSetting)
          && (stored.rememberLastDirectory === true || !storedDirectories
            || (storedDirectories.local === null && storedDirectories.remote.length === 0))) {
          return {
            browser: { ...stored.browser },
            rememberLastDirectory: stored.rememberLastDirectory === true,
            directories: stored.rememberLastDirectory === true && validDirectoryMemory(stored.directories)
              ? copyDirectoryMemory(stored.directories)
              : emptyDirectoryMemory(),
          };
        }
      }
    }
  } catch { /* Unavailable or corrupt display preferences use defaults. */ }
  return {
    browser: { ...DEFAULT_SFTP_PREFERENCES },
    rememberLastDirectory: false,
    directories: emptyDirectoryMemory(),
  };
}

export const useSftpPreferencesStore = defineStore("sftpPreferences", () => {
  const initial = load();
  const browser = ref(initial.browser);
  const rememberLastDirectory = ref(initial.rememberLastDirectory);
  const directories = ref(initial.directories);
  const directoryMemoryPersistenceFailed = ref(false);
  function hydrateCorePreferences(next: { browser: SftpBrowserPreferences; rememberLastDirectory: boolean }) {
    browser.value = { ...next.browser };
    rememberLastDirectory.value = next.rememberLastDirectory;
    if (!next.rememberLastDirectory) directories.value = emptyDirectoryMemory();
  }
  async function saveGlobal(next: StoredSftpPreferences) {
    if (corePreferencesEnabled()) {
      const expected = generalPreferences();
      const value = { browser: { ...next.browser }, rememberLastDirectory: next.rememberLastDirectory };
      if (!await saveApplicationPreferences("files", value, expected)) return false;
      browser.value = { ...next.browser };
      rememberLastDirectory.value = next.rememberLastDirectory;
      directories.value = copyDirectoryMemory(next.directories);
      // Directory memory is device-local and remains in the existing key.
      try {
        localStorage.setItem(SFTP_PREFERENCES_KEY, JSON.stringify({ version: 1, ...next }));
        directoryMemoryPersistenceFailed.value = false;
      } catch {
        // Core committed the global setting; only the device-local directory memory failed.
        directoryMemoryPersistenceFailed.value = true;
      }
      return true;
    }
    return persist(next);
  }
  function persist(next: StoredSftpPreferences) {
    try {
      localStorage.setItem(SFTP_PREFERENCES_KEY, JSON.stringify({
        version: 1,
        browser: next.browser,
        rememberLastDirectory: next.rememberLastDirectory,
        directories: next.rememberLastDirectory ? next.directories : emptyDirectoryMemory(),
      }));
    } catch { return false; }
    browser.value = { ...next.browser };
    rememberLastDirectory.value = next.rememberLastDirectory;
    directories.value = copyDirectoryMemory(next.rememberLastDirectory ? next.directories : emptyDirectoryMemory());
    directoryMemoryPersistenceFailed.value = false;
    return true;
  }
  async function replaceBrowser(next: SftpBrowserPreferences) {
    if (!validSftpBrowserPreferences(next)) return false;
    return saveGlobal({ browser: { ...next }, rememberLastDirectory: rememberLastDirectory.value, directories: directories.value });
  }
  function generalPreferences() {
    return { browser: { ...browser.value }, rememberLastDirectory: rememberLastDirectory.value };
  }
  async function replaceGeneralPreferences(next: ReturnType<typeof generalPreferences>, expected: ReturnType<typeof generalPreferences>) {
    if (!validSftpBrowserPreferences(next.browser) || typeof next.rememberLastDirectory !== "boolean"
      || JSON.stringify(generalPreferences()) !== JSON.stringify(expected)) return false;
    return saveGlobal({ ...next, directories: next.rememberLastDirectory ? directories.value : emptyDirectoryMemory() });
  }
  async function setRememberLastDirectory(enabled: boolean) {
    return saveGlobal({
      browser: browser.value,
      rememberLastDirectory: enabled,
      directories: enabled ? directories.value : emptyDirectoryMemory(),
    });
  }
  function rememberedLocalDirectory() {
    return rememberLastDirectory.value ? directories.value.local : null;
  }
  function rememberedRemoteDirectory(hostId: string) {
    const entry = rememberLastDirectory.value
      ? directories.value.remote.find((candidate) => candidate.hostId === hostId)
      : undefined;
    return entry ? [...entry.pathBytes] : null;
  }
  function rememberLocalDirectory(path: string) {
    if (!rememberLastDirectory.value || !validLocalDirectory(path)) return false;
    return persist({
      browser: browser.value,
      rememberLastDirectory: true,
      directories: { ...directories.value, local: path },
    });
  }
  function rememberRemoteDirectory(hostId: string, pathBytes: number[]) {
    if (!rememberLastDirectory.value || !validRememberedRemoteDirectory({ hostId, pathBytes })) return false;
    const remote = directories.value.remote.filter((entry) => entry.hostId !== hostId);
    remote.push({ hostId, pathBytes: [...pathBytes] });
    if (remote.length > maximumRememberedHostDirectories) remote.splice(0, remote.length - maximumRememberedHostDirectories);
    return persist({
      browser: browser.value,
      rememberLastDirectory: true,
      directories: { local: directories.value.local, remote },
    });
  }
  return {
    browser,
    rememberLastDirectory,
    directoryMemoryPersistenceFailed,
    hydrateCorePreferences,
    replaceBrowser,
    generalPreferences,
    replaceGeneralPreferences,
    setRememberLastDirectory,
    rememberedLocalDirectory,
    rememberedRemoteDirectory,
    rememberLocalDirectory,
    rememberRemoteDirectory,
  };
});
