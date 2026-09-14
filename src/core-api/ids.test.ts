import { describe, expect, it, vi } from "vitest";

import { createUuidV7 } from "./ids";

describe("createUuidV7", () => {
  it("encodes the timestamp, UUIDv7 version, and RFC variant", () => {
    const random = vi.spyOn(crypto, "getRandomValues").mockImplementation((value) => {
      (value as Uint8Array).fill(0xaa);
      return value;
    });
    const id = createUuidV7(0x0123_4567_89ab);
    expect(id).toBe("01234567-89ab-7aaa-aaaa-aaaaaaaaaaaa");
    random.mockRestore();
  });
});
