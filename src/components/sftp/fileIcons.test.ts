import { describe, expect, it } from "vitest";
import { File, FileArchive, FileBox, FileClock, FileCode2, FileCog, FileImage, FileKey2, FileQuestion, FileSymlink, Folder } from "lucide-vue-next";

import { sftpEntryIcon } from "./fileIcons";

describe("SFTP file icons", () => {
  it("uses the actual entry kind before the filename hint", () => {
    expect(sftpEntryIcon({ kind: "directory", displayName: "photos.jpg" })).toBe(Folder);
    expect(sftpEntryIcon({ kind: "symlink", displayName: "config.json" })).toBe(FileSymlink);
    expect(sftpEntryIcon({ kind: "other", displayName: "socket.txt" })).toBe(FileQuestion);
  });

  it.each([
    ["PHOTO.JPEG", FileImage],
    ["server.tar.gz", FileArchive],
    ["server.log.2.gz", FileArchive],
    ["server.log.2", FileClock],
    ["libssl.so.3", FileBox],
    [".env.production", FileCog],
    ["Dockerfile.production", FileCode2],
    ["service.mts", FileCode2],
    ["server.key", FileKey2],
    ["id_ed25519", FileKey2],
    ["unknown.something", File],
    ["no-extension", File],
    ["constructor", File],
  ])("classifies %s without reading the file", (displayName, icon) => {
    expect(sftpEntryIcon({ kind: "file", displayName })).toBe(icon);
  });
});
