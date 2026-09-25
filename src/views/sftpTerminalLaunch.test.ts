import { describe, expect, it } from "vitest";
import {
  createSftpTerminalLaunch,
  posixDirectoryCommand,
  shellDirectoryFromSftpPath,
  takeSftpTerminalLaunch,
} from "./sftpTerminalLaunch";

const bytes = (value: string) => Array.from(new TextEncoder().encode(value));

describe("SFTP terminal directory handoff", () => {
  it("accepts an absolute UTF-8 directory and consumes the operation once for its Host", () => {
    const operationId = createSftpTerminalLaunch("host-a", bytes("/srv/发布"));
    expect(operationId).not.toBeNull();
    expect(takeSftpTerminalLaunch(operationId!, "host-a")).toBe("/srv/发布");
    expect(takeSftpTerminalLaunch(operationId!, "host-a")).toBeNull();
  });

  it("rejects opaque non-UTF-8 and command-control paths before opening a terminal", () => {
    expect(shellDirectoryFromSftpPath([47, 255])).toBeNull();
    expect(shellDirectoryFromSftpPath(bytes("relative/path"))).toBeNull();
    expect(shellDirectoryFromSftpPath(bytes("/C:/Users/name"))).toBeNull();
    expect(shellDirectoryFromSftpPath(bytes("/safe\ncmd"))).toBeNull();
    expect(createSftpTerminalLaunch("host-a", [47, 255])).toBeNull();
  });

  it("quotes apostrophes as one POSIX argument and rejects a Host mismatch", () => {
    expect(posixDirectoryCommand("/srv/a'b;$(whoami)")).toBe("cd '/srv/a'\\''b;$(whoami)'\r");
    const operationId = createSftpTerminalLaunch("host-a", bytes("/srv/a'b"))!;
    expect(takeSftpTerminalLaunch(operationId, "host-b")).toBeNull();
    expect(takeSftpTerminalLaunch(operationId, "host-a")).toBeNull();
  });
});
