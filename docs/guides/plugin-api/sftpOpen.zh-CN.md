# sftpOpen

为已授权主机请求独立的 SFTP 根目录资源。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::SftpOpen { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "sftpOpen" | 是 | 判别标签；必须使用表中固定值 |
| `hostHandle` | String | 是 | Core 主机上下文提供的不透明引用 |
| `rootPath` | String | 是 | 请求批准的远程根路径 |
| `write` | bool | 是 | 申请写入范围；仍需对应能力和批准 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "sftp" | 是 | 判别标签；必须使用表中固定值 |
| `result` | [PluginSftpResult](./types.zh-CN.md#pluginsftpresult) | 是 | 嵌套结果联合；先检查 result.kind |

## 权限、时机与资源范围

能力提示：`sftpRead`。声明能力后仍须有当前有效授权及适用的受保护操作批准。

hostHandle 来自 Core 提供的主机上下文，不是 HostId 或 SSH session ID。Core 审核 rootPath 后授予根句柄；write=true 还需要 sftpWrite。连接独立于已有终端。

## 调用示例

示例中的 UUID 是格式占位值；必须换成前置 API 返回或 Core 上下文提供的对应句柄/任务标识。它不能直接执行，也不能跨插件或重启复用。

```json
{
  "callId": "example.sftpOpen",
  "operation": {
    "kind": "sftpOpen",
    "hostHandle": "019d0000-0000-7000-8000-000000000001",
    "rootPath": ".",
    "write": false
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="sftp"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[sftp](./sftp.zh-CN.md) · [resourceClose](./resourceClose.zh-CN.md)
