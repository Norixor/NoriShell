# taskCancel

使用最新 revision 请求取消任务及其资源。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::TaskCancel { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "taskCancel" | 是 | 判别标签；必须使用表中固定值 |
| `taskId` | [PluginWorkflowTaskId](./types.zh-CN.md#pluginworkflowtaskid) | 是 | 任务快照提供的不透明任务标识 |
| `expectedRevision` | [WireSequence](./types.zh-CN.md#wiresequence) | 是 | 回传最近读取的 revision；不要自行递增 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "task" | 是 | 判别标签；必须使用表中固定值 |
| `snapshot` | [PluginWorkflowTaskSnapshot](./types.zh-CN.md#pluginworkflowtasksnapshot) | 是 | 包含 task、steps、可选 result 的任务快照 |

## 权限、时机与资源范围

`describe` 不为本方法标注独立能力；调用仍受插件实例身份、所有权和调用上下文约束。

taskId 和 expectedRevision 均取自最新快照的 task。取消受所有权和 revision 检查；返回快照后检查 state 与 cleanupIncomplete，不把取消请求视为所有资源已释放。发生 conflict 时先 taskGet。

## 调用示例

示例中的 UUID 是格式占位值；必须换成前置 API 返回或 Core 上下文提供的对应句柄/任务标识。它不能直接执行，也不能跨插件或重启复用。

```json
{
  "callId": "example.taskCancel",
  "operation": {
    "kind": "taskCancel",
    "taskId": "019d0000-0000-7000-8000-000000000001",
    "expectedRevision": "1"
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="task"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[taskStart](./taskStart.zh-CN.md) · [taskGet](./taskGet.zh-CN.md) · [taskList](./taskList.zh-CN.md) · [taskResume](./taskResume.zh-CN.md)
