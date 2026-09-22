# 隔离 UI：位于 Core 桥接后的 HTML/CSS/JS

当普通声明式 document 不足以承载更丰富的包内页面时，使用隔离 UI。它不是向 NoriShell WebView 注入代码的方式。Wasm guest 仍先贡献一个小型宿主渲染入口，再请求 Core 打开 ZIP 内已校验的 HTML 资源。

## 页面流程

源码 Demo 的 `Initialize` 创建 `app.header.actions` dialog。点击 **Open isolated demo** 时，同时返回刷新后的 `ui.document` 和 `isolated-demo` surface 的 `ui.webview.open`。ZIP 将该 surface 映射到 `assets/isolated/isolated-demo.html`。下面 Rust output 是解释性节选：应保留真实函数的 import、已校验 request 与 document helper；它不是完整插件 handler。

```rust
Ok(vec![
    document(&request.request_id, &body)?,
    output(&request.request_id, "ui.webview.open", &json!({
        "surfaceId": "isolated-demo",
        "title": "NoriShell Isolated API Demo",
        "width": 760,
        "height": 620
    }))?,
])
```

Core 拥有隔离 WebView。页面初始禁用所有控件，等待父窗口发送携带 protocol 13 与 MessagePort 的一次 `norishell.bridge.ready` event。只有该 port 用于类型化请求与回复。下面 JavaScript 只是消息形状节选，不是可独立运行的完整页面：

```js
port.postMessage({
  type: 'norishell.api.request',
  call: { callId: 'isolated-demo.describe.1', operation: { kind: 'describe' } }
});
```

Demo 通过该 bridge 查询 API 描述、访问非秘密私有 storage、发起受保护 HTTP GET，并专门测试一个被拒绝的终端输入请求。页面没有直接网络、Tauri、文件、Vault、终端或宿主 DOM 访问。受保护请求可以进入 pending 状态；必须等待 Core 的类型化 reply，不能自行宣布成功。

| 看到的现象 | 所需输入与预期结果 |
| --- | --- |
| 有入口但未打开隔离窗口 | 点击必须同时返回刷新 document 与 `ui.webview.open`；确认请求的 `surfaceId` 与包内资源一致。 |
| 所有控件一直禁用 | 等待 Core 的单次 `norishell.bridge.ready` event，其中有 protocol 13 与一个 MessagePort。页面代码不能虚构 port。 |
| 网络控件处于 pending | 所需输入是 Core 对精确 HTTPS request 的用户审阅；页面必须等待类型化 reply。 |
| 终端测试被拒绝 | 这是预期 output：该包没有终端权限。 |

## 包内容与验证

隔离包包含 `manifest.json`、`plugin.wasm` 与 `assets/isolated/isolated-demo.html`。manifest 申请 `uiPanel`、`uiWebviewIsolated`、`storagePlugin` 和 `networkDomain`。

在 NoriShell 源码检出根目录运行：

```sh
python3 examples/plugins/isolated-demo/build.py --output /tmp/NoriShell-Isolated-API-Demo-1.0.2.zip
```

该脚本会构建 Wasm 并验证隔离页面 ABI，再创建 ZIP。它不安装包，也不能证明桌面 WebView、MessagePort、受保护提示或资源清理。请用 [UI document](../development/ui.zh-CN.md)、[调用 Core](../development/calling-api.zh-CN.md) 与 [打包](../development/packaging.zh-CN.md) 核对资源必须遵循的契约。
