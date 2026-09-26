# dataReview

一次受保护审阅中选择完整的本机或云端候选。插件先针对同一组 `DataSnapshot`、`DataInspect` 和已验证 GET 收据分别调用 `dataCompose`，获得两套完整候选，再调用本方法。Core 核对句柄、候选内容与来源，在本机受保护窗口展示两种选择的应用前后数量、差异、删除和是否需要上传。取消或验证失败均不批准后续写入。

通过 SDK `api_request(request_id, call_id, PluginApiOperation::DataReview { … })` 调用；核对 `PluginApiReply.outcome` 和 `value.kind="dataReview"`。[Broker 调用说明](../developers/development/calling-api.zh-CN.md)。

## 请求

`operation.kind="dataReview"`；`request: {profileId, categories, localSnapshotHandle, remoteInspectionHandle, baseReceiptHandle, localComposedHandle, remoteComposedHandle}`。五个句柄均为 Core 签发的不透明值；两套候选必须来自同一分类范围、快照及远端检查。插件不传确认结果或审批 token。

## 成功返回

`value.kind="dataReview"` 返回为选定方案新签发的 `composedHandle`、`source: local | remote` 与该候选的 `objects[]`；返回句柄不同于输入的两套候选句柄。Core 内部签发一次性审批，后续 `dataApply` / `dataExport` 仍核对当前 generation、范围和收据。插件根据所选候选是否等同已认证云端，沿 GET 收据直接应用，或先执行条件 PUT；不能把此返回当作远端或本机已提交。

操作结束、取消或失败时使用 [`dataRelease`](./dataRelease.zh-CN.md) 释放快照、检查、原有两套组合候选、选定后返回的 `composedHandle`、Blob 和收据句柄。PUT 已成功而本机应用或基线提交失败时，须分别报告云端已更新的事实和原始失败原因，不能报成整个同步均未发生。

[DTO 类型](./types.zh-CN.md) · [Broker 目录](./README.zh-CN.md)
