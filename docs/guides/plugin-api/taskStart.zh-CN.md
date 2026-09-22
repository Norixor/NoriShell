# taskStart

启动包中声明的工作流，并取得可跟踪的任务快照。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::TaskStart { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "taskStart" | 是 | 判别标签；必须使用表中固定值 |
| `workflowId` | String | 是 | 包内 workflow catalog 中的 id |
| `inputJson` | Option&lt;String&gt; | 否 | 按工作流约定编码的 JSON 字符串，仅进程内保留 |
| `fileScopeHandles` | Vec&lt;String&gt; | 是 | 用户预先选择的文件根句柄；默认空数组 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "task" | 是 | 判别标签；必须使用表中固定值 |
| `snapshot` | [PluginWorkflowTaskSnapshot](./types.zh-CN.md#pluginworkflowtasksnapshot) | 是 | 包含 task、steps、可选 result 的任务快照 |

## 权限、时机与资源范围

`describe` 不为本方法标注独立能力；调用仍受插件实例身份、所有权和调用上下文约束。

taskId 与 revision 取自任务快照；taskStart 的 workflowId 必须存在于包的 workflow catalog，示例采用 Workflow Demo 的 inspect-delay-inspect，自有插件需使用自己的声明。启动和恢复要求显式用户动作；取消仍受当前所有者和 revision 检查。inputJson、文件句柄映射与结果仅保留在进程中；重启后未完成任务为 interrupted，不重放旧 payload。taskResume 是新动作，不保证先前未知副作用可以安全重试。

## 调用示例

```json
{
  "callId": "example.taskStart",
  "operation": {
    "kind": "taskStart",
    "workflowId": "inspect-delay-inspect",
    "fileScopeHandles": []
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="task"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[taskGet](./taskGet.zh-CN.md) · [taskList](./taskList.zh-CN.md) · [taskCancel](./taskCancel.zh-CN.md) · [taskResume](./taskResume.zh-CN.md)
