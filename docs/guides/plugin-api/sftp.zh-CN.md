# sftp

通过根目录和条目句柄操作远程文件。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::Sftp { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "sftp" | 是 | 判别标签；必须使用表中固定值 |
| `operation` | [PluginSftpOperation](./types.zh-CN.md#pluginsftpoperation) | 是 | 嵌套判别联合；按 kind 选择字段 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "sftp" | 是 | 判别标签；必须使用表中固定值 |
| `result` | [PluginSftpResult](./types.zh-CN.md#pluginsftpresult) | 是 | 嵌套结果联合；先检查 result.kind |

## 权限、时机与资源范围

能力提示：`sftpRead`。声明能力后仍须有当前有效授权及适用的精确操作批准。

rootHandle 来自 sftpOpen；directoryHandle、entryHandle、precondition 来自 list/read。写操作还需 sftpWrite。每页最多 100 项，每块 16 KiB，暂存上传总量最多 64 MiB。只暴露常规文件与真实目录；不得构造远端路径或会话 ID。上传按 offset 顺序 chunk，再 commit，失败时 abort；关闭根资源也清理暂存。

## 调用示例

示例中的 UUID 是格式占位值；必须换成前置 API 返回或 Core 上下文提供的对应句柄/任务标识。它不能直接执行，也不能跨插件或重启复用。

```json
{
  "callId": "example.sftp",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "list",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "limit": 20
    }
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="sftp"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。

## 子操作

内层 `operation.kind` 选择以下分支。完整参数与返回对象字段见[类型参考](./types.zh-CN.md#pluginsftpoperation)。

| kind | 参数（? 表示可省略） | result.kind |
| --- | --- | --- |
| `list` | `rootHandle`, `directoryHandle?`, `childEntryHandle?`, `cursor?`, `limit` | `page` |
| `read` | `rootHandle`, `directoryHandle`, `entryHandle`, `offset`, `length` | `read` |
| `writeBinary` | `rootHandle`, `directoryHandle`, `entryHandle`, `precondition`, `dataBase64` | `written` |
| `uploadStart` | `rootHandle`, `directoryHandle`, `name`, `expectedBytes` | `uploadStarted` |
| `uploadReplaceStart` | `rootHandle`, `directoryHandle`, `entryHandle`, `precondition`, `expectedBytes` | `uploadStarted` |
| `uploadChunk` | `rootHandle`, `uploadHandle`, `offset`, `dataBase64` | `uploadProgress` |
| `uploadCommit` | `rootHandle`, `uploadHandle` | `uploaded` |
| `uploadAbort` | `rootHandle`, `uploadHandle` | `uploadAborted` |
| `createDirectory` | `rootHandle`, `directoryHandle`, `name` | `created` |
| `createEmptyFile` | `rootHandle`, `directoryHandle`, `name` | `created` |
| `renameNoReplace` | `rootHandle`, `sourceDirectoryHandle`, `sourceEntryHandle`, `sourcePrecondition`, `targetDirectoryHandle`, `targetName` | `renamed` |
| `remove` | `rootHandle`, `directoryHandle`, `entryHandle`, `precondition` | `removed` |

下面每项都是独立的 PluginApiCall。所有句柄、fingerprint、precondition 与 revision 必须换成前置查询结果；示例不可直接执行。

### list

```json
{
  "callId": "example.sftp.list",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "list",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "limit": 16
    }
  }
}
```

### read

```json
{
  "callId": "example.sftp.read",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "read",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "entryHandle": "019d0000-0000-7000-8000-000000000001",
      "offset": 0,
      "length": 16
    }
  }
}
```

### writeBinary

```json
{
  "callId": "example.sftp.writeBinary",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "writeBinary",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "entryHandle": "019d0000-0000-7000-8000-000000000001",
      "precondition": {
        "kind": "file",
        "size": 1,
        "modifiedAtUnixMs": 0
      },
      "dataBase64": "YQ=="
    }
  }
}
```

### uploadStart

```json
{
  "callId": "example.sftp.uploadStart",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "uploadStart",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "name": "renamed.txt",
      "expectedBytes": 1
    }
  }
}
```

### uploadReplaceStart

```json
{
  "callId": "example.sftp.uploadReplaceStart",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "uploadReplaceStart",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "entryHandle": "019d0000-0000-7000-8000-000000000001",
      "precondition": {
        "kind": "file",
        "size": 1,
        "modifiedAtUnixMs": 0
      },
      "expectedBytes": 1
    }
  }
}
```

### uploadChunk

```json
{
  "callId": "example.sftp.uploadChunk",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "uploadChunk",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "uploadHandle": "019d0000-0000-7000-8000-000000000001",
      "offset": 0,
      "dataBase64": "YQ=="
    }
  }
}
```

### uploadCommit

```json
{
  "callId": "example.sftp.uploadCommit",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "uploadCommit",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "uploadHandle": "019d0000-0000-7000-8000-000000000001"
    }
  }
}
```

### uploadAbort

```json
{
  "callId": "example.sftp.uploadAbort",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "uploadAbort",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "uploadHandle": "019d0000-0000-7000-8000-000000000001"
    }
  }
}
```

### createDirectory

```json
{
  "callId": "example.sftp.createDirectory",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "createDirectory",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "name": "renamed.txt"
    }
  }
}
```

### createEmptyFile

```json
{
  "callId": "example.sftp.createEmptyFile",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "createEmptyFile",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "name": "renamed.txt"
    }
  }
}
```

### renameNoReplace

```json
{
  "callId": "example.sftp.renameNoReplace",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "renameNoReplace",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "sourceDirectoryHandle": "019d0000-0000-7000-8000-000000000001",
      "sourceEntryHandle": "019d0000-0000-7000-8000-000000000001",
      "sourcePrecondition": {
        "kind": "file",
        "size": 1,
        "modifiedAtUnixMs": 0
      },
      "targetDirectoryHandle": "019d0000-0000-7000-8000-000000000001",
      "targetName": "renamed.txt"
    }
  }
}
```

### remove

```json
{
  "callId": "example.sftp.remove",
  "operation": {
    "kind": "sftp",
    "operation": {
      "kind": "remove",
      "rootHandle": "019d0000-0000-7000-8000-000000000001",
      "directoryHandle": "019d0000-0000-7000-8000-000000000001",
      "entryHandle": "019d0000-0000-7000-8000-000000000001",
      "precondition": {
        "kind": "file",
        "size": 1,
        "modifiedAtUnixMs": 0
      }
    }
  }
}
```


## 相关方法

[sftpOpen](./sftpOpen.zh-CN.md) · [resourceEvents](./resourceEvents.zh-CN.md) · [resourceClose](./resourceClose.zh-CN.md)
