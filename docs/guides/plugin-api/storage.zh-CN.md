# storage

读写插件私有的非秘密 KV、Blob、缓存和 schema 状态。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::Storage { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "storage" | 是 | 判别标签；必须使用表中固定值 |
| `operation` | [PluginStorageOperation](./types.zh-CN.md#pluginstorageoperation) | 是 | 嵌套判别联合；按 kind 选择字段 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "storage" | 是 | 判别标签；必须使用表中固定值 |
| `result` | [PluginStorageResult](./types.zh-CN.md#pluginstorageresult) | 是 | 嵌套结果联合；先检查 result.kind |

## 权限、时机与资源范围

能力提示：`storagePlugin`。声明能力后仍须有当前有效授权及适用的受保护操作批准。

不保存秘密。Base64 是标准编码，不带 data URL 前缀；KV/Blob 写入回传条目 revision，新建使用 0。schemaCommit 在一个事务中应用声明的 mutation，不运行迁移代码。缓存可能过期，缺少 entry 不是解码失败。storage 的 u64 revision 是 JSON 整数，区别于 WireSequence。

## 调用示例

```json
{
  "callId": "example.storage",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "kvGet",
      "key": "theme"
    }
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="storage"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。

## 子操作

内层 `operation.kind` 选择以下分支。完整参数与返回对象字段见[类型参考](./types.zh-CN.md#pluginstorageoperation)。

| kind | 参数（? 表示可省略） | result.kind |
| --- | --- | --- |
| `kvGet` | `key` | `kvValue` |
| `kvSet` | `key`, `valueBase64`, `expectedRevision` | `kvValue` |
| `kvDelete` | `key`, `expectedRevision` | `state` |
| `kvList` | `prefix`, `cursor?`, `limit` | `kvPage` |
| `blobRead` | `key`, `offset`, `length` | `blobRead` |
| `blobWrite` | `key`, `offset`, `dataBase64`, `expectedRevision` | `blobWritten` |
| `blobDelete` | `key`, `expectedRevision` | `state` |
| `cacheGet` | `key` | `cacheValue` |
| `cacheSet` | `key`, `valueBase64`, `ttlMs` | `cacheValue` |
| `cacheDelete` | `key` | `state` |
| `cacheClear` | — | `state` |
| `schemaGet` | — | `state` |
| `schemaCommit` | `expectedVersion`, `newVersion`, `mutations` | `state` |

下面每项都是独立的 PluginApiCall。所有句柄、fingerprint、precondition 与 revision 必须换成前置查询结果；示例不可直接执行。

### kvGet

```json
{
  "callId": "example.storage.kvGet",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "kvGet",
      "key": "theme"
    }
  }
}
```

### kvSet

```json
{
  "callId": "example.storage.kvSet",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "kvSet",
      "key": "theme",
      "valueBase64": "YQ==",
      "expectedRevision": 0
    }
  }
}
```

### kvDelete

```json
{
  "callId": "example.storage.kvDelete",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "kvDelete",
      "key": "theme",
      "expectedRevision": 0
    }
  }
}
```

### kvList

```json
{
  "callId": "example.storage.kvList",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "kvList",
      "prefix": "",
      "limit": 16
    }
  }
}
```

### blobRead

```json
{
  "callId": "example.storage.blobRead",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "blobRead",
      "key": "theme",
      "offset": 0,
      "length": 16
    }
  }
}
```

### blobWrite

```json
{
  "callId": "example.storage.blobWrite",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "blobWrite",
      "key": "theme",
      "offset": 0,
      "dataBase64": "YQ==",
      "expectedRevision": 0
    }
  }
}
```

### blobDelete

```json
{
  "callId": "example.storage.blobDelete",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "blobDelete",
      "key": "theme",
      "expectedRevision": 0
    }
  }
}
```

### cacheGet

```json
{
  "callId": "example.storage.cacheGet",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "cacheGet",
      "key": "theme"
    }
  }
}
```

### cacheSet

```json
{
  "callId": "example.storage.cacheSet",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "cacheSet",
      "key": "theme",
      "valueBase64": "YQ==",
      "ttlMs": 60000
    }
  }
}
```

### cacheDelete

```json
{
  "callId": "example.storage.cacheDelete",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "cacheDelete",
      "key": "theme"
    }
  }
}
```

### cacheClear

```json
{
  "callId": "example.storage.cacheClear",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "cacheClear"
    }
  }
}
```

### schemaGet

```json
{
  "callId": "example.storage.schemaGet",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "schemaGet"
    }
  }
}
```

### schemaCommit

```json
{
  "callId": "example.storage.schemaCommit",
  "operation": {
    "kind": "storage",
    "operation": {
      "kind": "schemaCommit",
      "expectedVersion": 0,
      "newVersion": 1,
      "mutations": []
    }
  }
}
```


## 相关方法

[credential](./credential.zh-CN.md) · [permissions](./permissions.zh-CN.md)
