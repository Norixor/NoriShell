# taskList

列出当前插件拥有的任务快照。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::TaskList { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "taskList" | 是 | 判别标签；必须使用表中固定值 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "tasks" | 是 | 判别标签；必须使用表中固定值 |
| `snapshots` | Vec&lt;[PluginWorkflowTaskSnapshot](./types.zh-CN.md#pluginworkflowtasksnapshot)&gt; | 是 | 当前插件可见的任务快照数组 |

## 权限、时机与资源范围

`describe` 不为本方法标注独立能力；调用仍受插件实例身份、所有权和调用上下文约束。

只返回当前插件拥有的快照，不接受插件身份过滤参数。重启后仍可看到持久化的任务摘要，但未完成任务成为 interrupted，进程内 result 不会恢复。

## 调用示例

```json
{
  "callId": "example.taskList",
  "operation": {
    "kind": "taskList"
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="tasks"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[taskStart](./taskStart.zh-CN.md) · [taskGet](./taskGet.zh-CN.md) · [taskCancel](./taskCancel.zh-CN.md) · [taskResume](./taskResume.zh-CN.md)
