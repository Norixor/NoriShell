export const secureWindowEn = {
  vault: {
    denied: "This Vault request is not allowed from this window.",
    changed: "The Vault or connection changed. Close this window and try again.",
    restartRequired: "Restart NoriShell before using the Vault again.",
    expired: "This Vault request expired. Open it again.",
    unavailable: "The Vault request is temporarily unavailable. Try again.",
  },
  credential: {
    denied: "This credential request is not allowed from this window.",
    invalidInput: "The credential details are invalid. Check them and try again.",
    expired: "This credential request expired. Open it again.",
    unavailable: "The credential request is temporarily unavailable. Try again.",
    failed: "The credential could not be saved. Check the Vault and try again.",
  },
};

export const secureWindowZhCN: typeof secureWindowEn = {
  vault: {
    denied: "此窗口无权处理该 Vault 请求。",
    changed: "Vault 或连接状态已变化，请关闭窗口后重试。",
    restartRequired: "请重启 NoriShell 后再使用 Vault。",
    expired: "Vault 请求已过期，请重新打开。",
    unavailable: "Vault 请求暂时不可用，请重试。",
  },
  credential: {
    denied: "此窗口无权处理该凭据请求。",
    invalidInput: "凭据内容无效，请检查后重试。",
    expired: "凭据请求已过期，请重新打开。",
    unavailable: "凭据请求暂时不可用，请重试。",
    failed: "无法保存凭据，请检查 Vault 后重试。",
  },
};
