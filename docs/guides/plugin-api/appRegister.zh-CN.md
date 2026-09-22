# appRegister

注册命令、可选快捷键与状态文字。

## 调用方式

使用 SDK `api_request(request_id, call_id, PluginApiOperation::AppRegister { … })`，从当前宿主请求的响应返回该 output。完整封装与 `BrokerResult` 解析见[调用 API](../developers/development/calling-api.zh-CN.md)。下表是 `operation` 内的字段；外层必须携带字符串 `callId`。

## 请求参数

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "appRegister" | 是 | 判别标签；必须使用表中固定值 |
| `registration` | [PluginAppRegistration](./types.zh-CN.md#pluginappregistration) | 是 | 命令与状态声明，详见对象类型表 |

## 返回值

成功时 `outcome.kind="completed"`，下表为 `outcome.value`。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `kind` | "appAccepted" | 是 | 判别标签；必须使用表中固定值 |

## 权限、时机与资源范围

`describe` 不为本方法标注独立能力；调用仍受插件实例身份、所有权和调用上下文约束。

必须从显式用户动作调用；最多 24 个命令与 8 个状态。actionId 必须指向已验证文档中启用的按钮。快捷键仅支持 Alt+Shift+大写字母，宿主绑定由用户启用；声明文件扩展名不授予文件权限。

## 调用示例

```json
{
  "callId": "example.appRegister",
  "operation": {
    "kind": "appRegister",
    "registration": {
      "commands": [
        {
          "id": "open-text",
          "label": "Open text file",
          "targetId": "app.header.actions",
          "actionId": "app-demo.pick",
          "pageId": null,
          "shortcut": "Alt+Shift+O",
          "fileExtensions": [
            "txt"
          ]
        }
      ],
      "statuses": [
        {
          "id": "ready",
          "text": "Ready"
        }
      ]
    }
  }
}
```

## 处理结果和失败

按 `callId` 关联回复，再匹配 `outcome.kind` 与 `value.kind="appAccepted"`，读取上表字段。

失败只读取稳定 `code`：`invalidRequest` 时修正字段、格式或边界；`permissionDenied`/`revoked` 时停止使用旧授权或句柄；需要受保护交互而处于后台时处理 `interactionRequired`，等待明确用户操作。不要自动重试未知结果的写操作。完整分支见[错误处理](../developers/development/errors.zh-CN.md)。


## 相关方法

[appNotify](./appNotify.zh-CN.md) · [appNavigate](./appNavigate.zh-CN.md) · [filePick](./filePick.zh-CN.md)
