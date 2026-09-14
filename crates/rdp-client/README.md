# NoriShell RDP 引擎

Core 调用 `run(BoxedDesktopIo, RdpOptions, Receiver<EngineCommand>, EngineControl, EventSink)`。
连接路径由 Core 提供；引擎不会解析 DNS、打开另一个 socket、寻找 KDC 或访问系统剪贴板。

- `server_name` 必须是真实目标的 DNS 名或 IP，不能填 SSH 隧道的本机端点。
- `certificate_approval` 缺省 `None`：系统信任根和公开 PKI 校验不通过即拒绝。只有 UnknownIssuer 可进入本次证书审批；叶证书必须可解析且在有效期内；该分支允许 Windows 默认 CN-only 或名称不匹配证书通过本次精确指纹人工确认身份，不把 CN 当作目标名，也不放开系统信任链的名称不匹配错误，TLS 1.2/1.3 持钥签名始终交给 rustls 验证。握手只建立加密通道，审批成功前不发送 CredSSP 或登录材料。Core 回调在受保护界面核对 `server_name` 与 `sha256_fingerprint`，不得缓存决定为全局信任。
- CredSSP 固定 NTLM（KerberosConfig=None）；意外的 SSPI 外部网络请求直接拒绝。支持 TLS 登录和 NLA 用户名/密码/域，不启用智能卡、音频、磁盘、文件、打印机、USB 或 UDP 通道。
- `password` 为 Zeroizing，连接完成立即释放 options；重激活 factory 使用无凭据配置。IronRDP/SSPI 内部可能有普通 String 副本，本 crate 不声称能清除第三方内部所有临时副本。
- `focus_epoch` 变化优先释放键鼠；出队和每次真实 writer 前重验 epoch。背压写入中失焦意味着可能部分送达，直接失败并关闭 transport，绝不重放；stop 独立取消所有等待。初始化包含审批最多 180 秒，单次写入最多 10 秒。
- framebuffer 发布完整 RGBA 快照；EventSink 只替换当前投影，禁止把每帧追加到队列。尺寸上限来自 desktop-protocol。线协议单 PDU 4 MiB，SVC/DVC 重组 4 MiB，FastPath 图形重组 16 MiB。首版不协商 bulk compression。
- 输入 scan_code 使用低 8 位 Set-1 扫描码和第 8 位 E0 标记；鼠标按钮为 RFB 顺序 bit0 左、bit1 中、bit2 右。Unicode 文本用成对按下/释放并分块编码。resize 发送 Display Control，服务端完成重激活后才更新画面尺寸；不支持通道时返回 UnsupportedOperation，不伪造尺寸变化。
- 文本剪贴板只有连接选项明确启用后建立 CLIPRDR。向远端只发布调用方明确提交的文本；远端 Unicode 文本仅作为有界事件返回 Core，由 Core 决定展示/复制，不直接写入系统剪贴板。

测试使用内存 duplex 验证真实 rustls 握手、未知证书缺省拒绝和指纹审批、旧焦点拒绝、背压失焦、停止、扫描码状态和分片上限。真实 RDP 服务的认证、首帧、重激活、剪贴板与 macOS/Windows 桌面验收仍需目标环境。
