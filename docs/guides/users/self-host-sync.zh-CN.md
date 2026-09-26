# 自建同步：部署服务端并安装插件

从 [NoriShell Releases](https://github.com/Norixor/NoriShell/releases) 分别下载 `NoriShell_SelfHostSync_Server_<sync-version>.zip` 和 `NoriShell_SelfHostSync_Plugin_<sync-version>.zip`。两包使用相同的 Sync 版本；它可与 NoriShell 应用版本不同。服务端 ZIP 包含 macOS、Windows、Linux 的 x64/ARM64 可执行文件和 `README.md`。插件 ZIP **不要解压**，稍后直接导入应用。源码见 [`example/self-host-sync`](../../../example/self-host-sync/readme.md)。

## 1. 配置服务端

1. 解压服务端 ZIP，选取与服务器系统和架构对应的可执行文件。以下以 Linux x64 为例，在终端运行：

   ```sh
   ./norishell-sync-server_linux_x64
   ```

2. 首次启动时按提示输入监听端口（默认 `8787`），再输入并确认至少 12 字节的**同步服务密码**。密码输入不可见。服务默认只监听 `127.0.0.1`，配置和数据保存在当前用户的 `NoriShell/self-host-sync` 配置目录；再次运行同一程序会读取已有配置。服务在前台运行，关闭终端或按 Ctrl+C 会停止。
3. 如需让局域网其他设备直连，启动时显式指定监听地址，例如 `./norishell-sync-server_linux_x64 --listen 0.0.0.0:8787`。在插件中填写服务器的实际 IP 和端口，例如 `192.168.1.10:8787`，**不要填写 `0.0.0.0`**。HTTP 会明文传输服务密码、令牌和请求元数据；公网或不可信网络应使用 HTTPS 反向代理，并让服务继续监听本机地址。HTTPS IP 地址须有受信任且覆盖该 IP 的证书。代理配置、`--data-dir`、开机托管和备份方法见服务端 ZIP 中的 `README.md`。

可在浏览器打开服务端地址（例如 `http://127.0.0.1:8787/`），用同一服务密码登录，只读查看密文包是否存在、修订号和最近写入时间。服务端不展示同步内容明文。请备份整个服务端数据目录，且不要在同一目录同时运行两个服务端进程。

### 后台运行服务端

Linux 建议用 systemd 管理。下面以可执行文件位于 `/opt/norishell-sync/`、数据目录为 `/var/lib/norishell-sync`、运行用户为 `norishell-sync` 为例。`User=norishell-sync` 要求系统中**已经存在这个用户**；新安装可以先创建用户和私有数据目录，再以该用户和**同一个 `--data-dir`** 在终端前台运行一次，设置服务密码，按 Ctrl+C 停止：

```sh
sudo useradd --system --user-group --no-create-home --home-dir /var/lib/norishell-sync --shell /usr/sbin/nologin norishell-sync
sudo install -d -o norishell-sync -g norishell-sync -m 700 /var/lib/norishell-sync
sudo -u norishell-sync /opt/norishell-sync/norishell-sync-server_linux_x64 --data-dir /var/lib/norishell-sync
```

如果用户已存在，不要重复执行 `useradd`。已有服务必须把 `--data-dir` 改成**当前实际使用的目录**，核对服务用户对目录及文件的权限，不要新建空目录替代原数据；切换到 systemd 前停止手动启动的进程。

将以下内容保存为 `/etc/systemd/system/norishell-sync.service`，并按实际路径和用户修改：

```ini
[Unit]
Description=NoriShell self-hosted sync
After=network.target

[Service]
Type=simple
User=norishell-sync
ExecStart=/opt/norishell-sync/norishell-sync-server_linux_x64 --data-dir /var/lib/norishell-sync --listen 127.0.0.1:8787
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now norishell-sync
systemctl status norishell-sync
journalctl -u norishell-sync -f
```

若 `systemctl status` 显示 `status=217/USER`，先用 `systemctl cat norishell-sync` 查看实际 `User=`，再用 `getent passwd <用户名>` 核对该用户是否存在。这表示 systemd 尚未成功切换运行用户，服务端程序还未启动；修正用户名或按上面的步骤创建专用用户后，执行 `sudo systemctl reset-failed norishell-sync` 和 `sudo systemctl start norishell-sync`。随后再检查数据目录权限与日志。

这样服务会开机启动，异常退出后由 systemd 重启。若使用 HTTPS 反向代理，保留 `127.0.0.1:8787` 监听地址。macOS 可用 launchd，Windows 可用任务计划程序设置开机启动与失败重试；当前 Windows 可执行文件不实现原生 Windows Service。各平台均应先初始化密码，并避免两个进程共用同一数据目录。更多运维细节见[服务端说明](../../../example/self-host-sync/server/README.md)。

## 2. 导入并连接插件

1. 在 NoriShell **插件**页面导入尚未解压的 `NoriShell_SelfHostSync_Plugin_<sync-version>.zip`，审阅并授予页面与 `sshSync` 权限。新版插件要求 Core API 至少 1.88；若提示 Core API 不兼容，先升级应用，再导入匹配的插件 ZIP。
2. 打开新增的**同步**页面，点击“填写服务器地址”；也可在该页面点击“设置 → 服务器地址与插件设置”。填写可访问的 `http://`、`https://` 地址或 `IP:端口`。纯 `IP:端口` 按 HTTP 处理，纯域名按 HTTPS 处理。
3. 点击“连接 / 登录”，用固定账户名 `owner` 和第 1 步设置的同步服务密码登录。密码通过 NoriShell 的安全表单提交，插件 Wasm 不读取密码或令牌。登录失败时先核对地址、网络、服务端进程和密码。

## 3. 完成首次同步

本节描述新版插件的目标操作流程，尚待原生验收。点击“立即同步”；需要用户决定时，插件必须展示真实可用的审阅入口。新设备先创建或解锁本机 Vault；若远端已有加密数据，还需输入最初上传设备所用 Vault 密码来恢复同步密钥。本机 Vault 密码可以不同。服务密码只用于登录服务端。页面打开时只查询登录状态，不弹出授权或恢复窗口。

自建同步插件只从 Core 选择三类业务数据：可移植 SSH 主机（含连接所需的身份与规则）、已保存的凭据，以及 RDP/VNC 远程桌面配置。应用偏好保留在本机，不进入同步包。它传输独立的端到端加密交换包，**不会上传完整 Vault 文件、Vault 密码、设备自动解锁材料、Known Hosts、Agent/FIDO 或本机外部文件路径**。

旧 Core 专属自动调度正在迁移为插件自行调度。新版自动同步行为尚未验收；新候选通过原生验收前，请使用明确点击的同步操作。

页面显示“需要审阅”且没有具体错误时，点击“审阅并同步”继续。插件中的“冲突采用本机/云端”只决定冲突项目的内容；随后 Core 可能另行确认是否将已核验的结果写入本机，并说明本次云端是否已经写入。取消本机写入不会撤销已完成的云端上传。若显示具体错误，则按错误说明处理；本机数据归属、同步密钥绑定或恢复对象冲突不会自动覆盖数据。

进入同步页会查询登录状态并显示上次成功读取的云端列表；点击“刷新远端状态”才会请求最新数据。网络或服务暂时不可用时，已有列表仍可浏览，并标明上次成功获取的 UTC 时间；它是旧快照，不能代表当前云端状态。刷新失败不会清除有效的登录会话，也无需因此重新登录。重启后可读取同一服务地址下保留的非秘密列表；本机 Vault 仍须解锁才能重新拉取或同步，首次成功读取前没有旧列表。同步网络授权的“记住相同操作”覆盖同一网络目标下的读取与上传；请求内容变化不会重新询问，协议、主机、端口、解析目标或插件包变化时需重新批准。直接更新已安装插件时，新包需重新获得权限；若账号服务配置完全一致，Core 可保留有效登录会话并迁移插件私有缓存。卸载后重新导入不同签名包不会继承原包私有登录与缓存。配置变化、会话失效或旧 Core 专有缓存无法转换时，仍需重新登录或在线刷新以建立新版列表。

同步删除主机时，其已保存转发规则也会在同一事务中删除，沿用上述删除设置；需要确认时，窗口会列出关联规则。运行中的转发会话不会随规则删除而停止。

更换服务密码会撤销现有会话。具体运维、备份和 API 契约见[服务端说明](../../../example/self-host-sync/server/README.md)。
