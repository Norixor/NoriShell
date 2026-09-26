# credential

创建、列出或撤销插件自有的非明文凭据引用。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::Credential { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "credential" | 是 | 判别标签；必须使用表中固定值 |
| `operation` | [PluginCredentialOperation](./types.zh-CN.md#plugincredentialoperation) | 是 | 嵌套判别联合；按 kind 选择字段 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "credential" | 是 | 判别标签；必须使用表中固定值 |
| `result` | [PluginCredentialResult](./types.zh-CN.md#plugincredentialresult) | 是 | 嵌套结果联合；先检查 result.kind |

## 权限、时机与资源范围

能力提示：`credentialsPlugin`。声明能力后仍须有当前有效授权及适用的受保护操作批准。

create 只提交 label、origin、注入方式和幂等信息；秘密由 Core 受保护窗口接收。list 返回 handle/revision/state，不能读取秘密。仅 ready 引用可用于 networkStart 的 credential；origin 必须匹配。

## 调用示例

```json
{
  "callId": "example.credential",
  "operation": {
    "kind": "credential",
    "operation": {
      "kind": "list"
    }
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="credential"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。

## 子操作

内层 `operation.kind` 选择以下分支。完整参数与返回对象字段见[类型参考](./types.zh-CN.md#plugincredentialoperation)。

| kind | 参数（? 表示可省略） | result.kind |
| --- | --- | --- |
| `create` | `operationId`, `idempotencyKey`, `label`, `target` | `created` |
| `list` | — | `list` |
| `revoke` | `handle`, `expectedRevision` | `revoked` |

下面每项都是独立的 PluginApiCall。所有句柄、fingerprint、precondition 与 revision 必须换成前置查询结果；示例不可直接执行。

### create

```json
{
  "callId": "example.credential.create",
  "operation": {
    "kind": "credential",
    "operation": {
      "kind": "create",
      "operationId": "credential-create",
      "idempotencyKey": "credential-create-1",
      "label": "Example service",
      "target": {
        "origin": "https://example.test",
        "injection": {
          "kind": "bearer"
        }
      }
    }
  }
}
```

### list

```json
{
  "callId": "example.credential.list",
  "operation": {
    "kind": "credential",
    "operation": {
      "kind": "list"
    }
  }
}
```

### revoke

```json
{
  "callId": "example.credential.revoke",
  "operation": {
    "kind": "credential",
    "operation": {
      "kind": "revoke",
      "handle": "019d0000-0000-7000-8000-000000000001",
      "expectedRevision": "1"
    }
  }
}
```


## 相关方法

[networkStart](./networkStart.zh-CN.md) · [permissions](./permissions.zh-CN.md)
