interface SftpTerminalLaunch {
  hostId: string;
  directory: string;
  expiresAt: number;
}

const launches = new Map<string, SftpTerminalLaunch>();
const decoder = new TextDecoder("utf-8", { fatal: true });
const encoder = new TextEncoder();

export function shellDirectoryFromSftpPath(bytes: readonly number[]): string | null {
  if (!bytes.length || bytes.length > 4096 || bytes.some((byte) => !Number.isInteger(byte) || byte < 0 || byte > 255)) return null;
  try {
    const directory = decoder.decode(new Uint8Array(bytes));
    if (!directory.startsWith("/") || /^\/[A-Za-z]:(?:\/|$)/u.test(directory)
      || [...directory].some((character) => {
      const codePoint = character.codePointAt(0)!;
      return codePoint < 32 || codePoint === 127 || codePoint === 0x2028 || codePoint === 0x2029;
    })) return null;
    const roundTrip = encoder.encode(directory);
    return roundTrip.length === bytes.length && roundTrip.every((byte, index) => byte === bytes[index])
      ? directory
      : null;
  } catch { return null; }
}

export function posixDirectoryCommand(directory: string): string {
  return `cd '${directory.replaceAll("'", "'\\''")}'\r`;
}

export function createSftpTerminalLaunch(hostId: string, pathBytes: readonly number[]): string | null {
  const directory = shellDirectoryFromSftpPath(pathBytes);
  if (!hostId || directory === null) return null;
  const now = Date.now();
  for (const [id, launch] of launches) if (launch.expiresAt <= now) launches.delete(id);
  const operationId = crypto.randomUUID();
  launches.set(operationId, { hostId, directory, expiresAt: now + 60_000 });
  return operationId;
}

export function takeSftpTerminalLaunch(operationId: string, hostId: string): string | null {
  const launch = launches.get(operationId);
  launches.delete(operationId);
  return launch?.hostId === hostId && launch.expiresAt > Date.now() ? launch.directory : null;
}
