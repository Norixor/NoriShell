export const applicationPreferenceErrorsZhCN = {
  conflict: "设置已在其他窗口改变。请重新打开设置页，读取最新值后再试。",
  invalidInput: "设置内容无效。请检查输入值后重试。",
  persistenceUnavailable: "本机设置存储暂不可用。请检查磁盘空间与访问权限后重试。",
  unsupportedSchema: "当前版本无法读取此设置格式。请更新应用后重试。",
  requiresReconciliation: "设置存储需要重新协调。请重新启动应用并检查设置。",
  unknown: "设置未保存。请重试；若持续失败，请查看诊断信息。",
} as const;

export const applicationPreferenceErrorsEn = {
  conflict: "Settings changed in another window. Reopen Settings to load the latest values, then retry.",
  invalidInput: "The setting is invalid. Check the value and retry.",
  persistenceUnavailable: "Local settings storage is unavailable. Check disk space and permissions, then retry.",
  unsupportedSchema: "This app version cannot read the settings format. Update the app and retry.",
  requiresReconciliation: "Settings storage needs reconciliation. Restart the app and check the settings.",
  unknown: "The setting was not saved. Retry; if it continues to fail, check diagnostics.",
} as const;
