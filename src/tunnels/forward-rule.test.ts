import { describe, expect, it } from "vitest";

import { buildForwardRule, ruleListener, ruleTarget } from "./forward-rule";

describe("forward rule view model", () => {
  it("builds local rules and rejects zero listen ports for saved templates", () => {
    expect(buildForwardRule({
      hostId: "host",
      kind: "local",
      bindAddress: "127.0.0.1",
      listenPort: "8022",
      targetHost: "db.internal",
      targetPort: "5432",
    })).toEqual({
      kind: "local",
      hostId: "host",
      localBindAddress: "127.0.0.1",
      localListenPort: 8022,
      remoteTargetHost: "db.internal",
      remoteTargetPort: 5432,
    });
    expect(buildForwardRule({
      hostId: "host",
      kind: "dynamic",
      bindAddress: "127.0.0.1",
      listenPort: "0",
      targetHost: "",
      targetPort: "",
    })).toBeNull();
  });

  it("keeps dynamic targets absent", () => {
    const rule = buildForwardRule({
      hostId: "host",
      kind: "dynamic",
      bindAddress: "127.0.0.1",
      listenPort: "1080",
      targetHost: "ignored",
      targetPort: "443",
    });
    expect(rule).toEqual({
      kind: "dynamic",
      hostId: "host",
      localBindAddress: "127.0.0.1",
      localListenPort: 1080,
    });
    expect(rule && ruleListener(rule)).toBe("127.0.0.1:1080");
    expect(rule && ruleTarget(rule)).toBeNull();
  });
});
