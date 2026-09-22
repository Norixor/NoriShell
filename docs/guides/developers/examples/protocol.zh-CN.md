# Protocol Demo：framed TCP 终端 provider

Protocol Demo 面向要构建协议 provider 的作者，不是普通面板示例。它在 `assets/protocols.json` 中声明固定 `framedTcp` provider，并申请 `uiPanel`、`terminalProvider` 与 `networkDomain`。

可见的 **Open framed TCP terminal** 动作输出一个类型化 `protocolOpen` request，不会自行创建 socket。Core 启动被认领的 terminal 后，guest 接收 provider event。下面顺序解释真实 provider lifecycle，不是可独立运行的 guest 代码：

```text
Connect -> networkStart(tcp endpoint)
Opened -> Ready
Input -> networkSend（frame type 0x01）
Resize -> networkSend（frame type 0x02）
Data -> 从 frame type 0x81 取 terminal bytes
Close -> 只丢弃该连接的 guest state
```

frame 格式为四字节 big-endian 的 `(type + payload)` 长度，之后是 type byte 与不透明 payload。guest 为每个 `connectionHandle` 保留独立 partial-frame buffer，不会重新解释 UTF-8 或 ANSI 数据。Core 拥有 TCP resource，并在通知 guest 前关闭它，因此 Close event 不能用来重新取得 broker call。

| 看到的现象 | 所需输入与预期结果 |
| --- | --- |
| 没有打开 terminal | 从可见动作开始，并在 Core 流程中提供 provider 配置的 TCP endpoint；单独 UI action 只应输出 `protocolOpen`。 |
| Input 或 resize 被忽略 | Core 报告 `Opened` 后，预期 input 用 type `0x01`、resize 用 type `0x02`，通过类型化 `networkSend` request 发出。 |
| terminal bytes 看起来损坏 | 保留每个 handle 的 partial-frame buffer，只转发 type `0x81` payload bytes，不做 UTF-8/ANSI 转换。 |
| 一个连接关闭了另一个 | 确认 state 以 `connectionHandle` 为键；Close 只丢弃匹配的 guest state。 |

在 NoriShell 源码检出根目录构建可复现的本地候选包：

```sh
python3 examples/plugins/protocol-demo/build.py \
  --output /tmp/NoriShell-Framed-TCP-Protocol-Demo-1.0.3.zip
```

脚本会通过生产 persistent Wasm runtime 运行 loopback fixture，检查 input、resize、分段 ANSI/UTF-8 byte frame、两个独立 handle 与旧 handle close 隔离，再校验 ZIP。它不会安装 ZIP，也不会启动桌面应用。桌面 terminal lifecycle 与受保护审批仍是独立验收。改造前请看 [调用 Core](../development/calling-api.zh-CN.md)、[错误](../development/errors.zh-CN.md) 与 [打包](../development/packaging.zh-CN.md)。
