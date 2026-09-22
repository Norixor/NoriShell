# filePick

打开原生选择器并申请精确的本地文件访问范围。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::FilePick { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "filePick" | 是 | 判别标签；必须使用表中固定值 |
| `pickerKind` | [PluginFilePickerKind](./types.zh-CN.md#pluginfilepickerkind) | 是 | file 或 directory 原生选择模式 |
| `access` | [PluginFileAccessRequest](./types.zh-CN.md#pluginfileaccessrequest) | 是 | 各访问位独立，省略默认 false |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "filePicked" | 是 | 判别标签；必须使用表中固定值 |
| `rootHandle` | String | 是 | filePick 或 sftpOpen 返回的根范围引用 |
| `label` | String | 是 | 供用户识别的显示文字 |

## 权限、时机与资源范围

能力提示：`localFiles`。声明能力后仍须有当前有效授权及适用的精确操作批准。

只能通过显式用户操作打开原生选择器；access 的各布尔字段省略时为 false。文件句柄使用空 relativePath，目录句柄使用相对路径；不能提交任意本地绝对路径替代选择。

## 调用示例

```json
{
  "callId": "example.filePick",
  "operation": {
    "kind": "filePick",
    "pickerKind": "file",
    "access": {
      "read": true
    }
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="filePicked"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[file](./file.zh-CN.md) · [taskStart](./taskStart.zh-CN.md)
