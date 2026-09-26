# file

在已选择的本地文件范围内读写、列举和监听。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::File { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "file" | 是 | 判别标签；必须使用表中固定值 |
| `operation` | [PluginFileOperation](./types.zh-CN.md#pluginfileoperation) | 是 | 嵌套判别联合；按 kind 选择字段 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "file" | 是 | 判别标签；必须使用表中固定值 |
| `result` | [PluginFileResult](./types.zh-CN.md#pluginfileresult) | 是 | 嵌套结果联合；先检查 result.kind |

## 权限、时机与资源范围

能力提示：`localFiles`。声明能力后仍须有当前有效授权及适用的受保护操作批准。

rootHandle 来自 filePick。单次读写块最多 16 KiB，物化文件最多 64 MiB，列表最多 100 项。修改现有文件必须回传最新 fingerprint；write 省略 expectedFingerprint 仅允许新建。rename 不覆盖目标。watchStart 返回独立事件句柄。

## 调用示例

示例中的 UUID 是格式占位值；必须换成前置 API 返回或 Core 上下文提供的对应句柄/任务标识。它不能直接执行，也不能跨插件或重启复用。

```json
{
  "callId": "example.file",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "read",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "relativePath": "",
      "offset": 0
    }
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="file"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。

## 子操作

内层 `operation.kind` 选择以下分支。完整参数与返回对象字段见[类型参考](./types.zh-CN.md#pluginfileoperation)。

| kind | 参数（? 表示可省略） | result.kind |
| --- | --- | --- |
| `read` | `rootHandle`, `relativePath`, `offset` | `read` |
| `write` | `rootHandle`, `relativePath`, `expectedFingerprint?`, `offset`, `dataBase64`, `finalSize?` | `written` |
| `list` | `rootHandle`, `relativePath`, `cursor?`, `limit` | `listed` |
| `rename` | `rootHandle`, `fromPath`, `toPath`, `expectedFingerprint` | `renamed` |
| `remove` | `rootHandle`, `relativePath`, `expectedFingerprint`, `recursive` | `removed` |
| `watchStart` | `rootHandle`, `relativePath`, `intervalMs` | `watchStarted` |

下面每项都是独立的 PluginApiCall。所有句柄、fingerprint、precondition 与 revision 必须换成前置查询结果；示例不可直接执行。

### read

```json
{
  "callId": "example.file.read",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "read",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "relativePath": "notes.txt",
      "offset": 0
    }
  }
}
```

### write

```json
{
  "callId": "example.file.write",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "write",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "relativePath": "notes.txt",
      "offset": 0,
      "dataBase64": "YQ=="
    }
  }
}
```

### list

```json
{
  "callId": "example.file.list",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "list",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "relativePath": "notes.txt",
      "limit": 16
    }
  }
}
```

### rename

```json
{
  "callId": "example.file.rename",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "rename",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "fromPath": "notes.txt",
      "toPath": "renamed.txt",
      "expectedFingerprint": "fingerprint-from-read"
    }
  }
}
```

### remove

```json
{
  "callId": "example.file.remove",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "remove",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "relativePath": "notes.txt",
      "expectedFingerprint": "fingerprint-from-read",
      "recursive": false
    }
  }
}
```

### watchStart

```json
{
  "callId": "example.file.watchStart",
  "operation": {
    "kind": "file",
    "operation": {
      "kind": "watchStart",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "relativePath": "notes.txt",
      "intervalMs": 1000
    }
  }
}
```


## 相关方法

[filePick](./filePick.zh-CN.md) · [resourceEvents](./resourceEvents.zh-CN.md) · [resourceClose](./resourceClose.zh-CN.md)
