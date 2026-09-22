# 资源、权限与状态管理

调用宿主能力前，先声明插件需要的 capability；运行时再按操作取得用户批准。把资源句柄、任务状态和持久数据分开管理，可以避免禁用、撤权或重启后重复执行旧操作。

## 两层权限分别控制什么

| 层级 | 由谁决定 | 插件作者怎样使用 |
| --- | --- | --- |
| Manifest capability | 插件声明，用户通过宿主批准 | 只申请实现功能所需的能力；声明本身不授权 |
| 具体操作批准 | Core 根据这次操作的目标、参数和上下文决定 | 通过明确用户动作触发请求，让宿主展示实际目标与风险 |
| 已记住的操作 | 用户在宿主提示中选择并设置有效期 | 用 `permissions` 读取非秘密摘要；仍要通过当前 capability 与执行检查 |

“始终允许”不是插件自行保存的布尔值。它只适用于宿主识别出的精确操作，用户可撤销或让其过期。不要自行展示一个勾选框来替用户批准文件、网络或进程访问。

| 管理动作 | API | 正确的后续处理 |
| --- | --- | --- |
| 查看方法和所需 capability | [describe](../../plugin-api/describe.zh-CN.md) | 根据 `availability` 展示功能，按 `limits` 控制请求大小 |
| 查看当前权限和操作摘要 | [permissions](../../plugin-api/permissions.zh-CN.md) | 保存当前 `policyRevision` 以便执行管理操作 |
| 请求 capability | [permissionRequest](../../plugin-api/permissionRequest.zh-CN.md) | 此方法只允许宿主声明式入口；Wasm 直接调用返回 `unsupported`。普通作者通过安装/管理流程取得批准 |
| 撤销一项操作记录 | [permissionRevoke](../../plugin-api/permissionRevoke.zh-CN.md) | 携带读取到的 revision，冲突时刷新摘要 |
| 清除当前包拥有的操作记录 | [permissionsForget](../../plugin-api/permissionsForget.zh-CN.md) | 刷新权限列表；不会因此扩大或替换 capability grant |

## 资源的完整使用顺序

```text
用户触发操作
  → 申请打开资源
  → 得到不透明 handle
  → 读取事件并更新界面
  → 完成、取消或失败
  → 关闭资源，清除实例中的 handle
```

| 阶段 | 使用的方法 | 要检查的事实 |
| --- | --- | --- |
| 创建 | `networkStart`、`processStart`、`remoteExecStart`、`serialOpen`、`timerStart`、`subscriptionStart` 等 | 返回种类是否正确，是否取得当前调用的 handle |
| 发送或继续 | 对应 `*Send` 或资源 API | 本地接受请求不代表远端已收到或执行 |
| 接收 | [resourceEvents](../../plugin-api/resourceEvents.zh-CN.md) | 事件顺序、数据上限、错误、退出和关闭状态；必要时处理背压 |
| 查询当前拥有的资源 | [resourcesList](../../plugin-api/resourcesList.zh-CN.md) | 只使用当前实例仍拥有的有效资源 |
| 结束 | [resourceClose](../../plugin-api/resourceClose.zh-CN.md) | 显式关闭并停止发出后续写入；失败或不确定结果应显示给用户 |

句柄是不透明字符串，不能从磁盘路径、设备名、主机 ID 或其他插件的输出推导。文件与 SFTP 也有自己的范围句柄和关闭操作，请按 [file](../../plugin-api/file.zh-CN.md) 与 [sftp](../../plugin-api/sftp.zh-CN.md) 的子操作说明使用。

## 什么能持久保存

| 数据 | 应放在哪里 | 重启后如何处理 |
| --- | --- | --- |
| 当前界面、等待调用、临时结果 | Wasm 实例内的有界状态 | 重新初始化，不假定上一个实例还存在 |
| 用户的非秘密偏好与插件业务数据 | [storage](../../plugin-api/storage.zh-CN.md) | 读取当前 revision，用 CAS 提交修改 |
| 密码、token 等凭据 | [credential](../../plugin-api/credential.zh-CN.md) 的宿主托管流程 | 只保留允许暴露的不透明引用和状态，不把秘密放入 storage |
| 网络、文件、串口、进程 handle | 当前操作的内存状态 | 不持久化，不跨实例复用 |
| 工作流任务 | [taskGet](../../plugin-api/taskGet.zh-CN.md)、[taskList](../../plugin-api/taskList.zh-CN.md) 返回的任务状态 | 重启后未完成任务可能是 `interrupted`；读取状态后让用户决定下一步 |

`storage` 的并发更新冲突应重新读取再处理，不能拿旧 revision 覆盖别人刚保存的数据。task 的输入、预选文件映射和运行回复并不作为可自动重放的持久 payload；`taskResume` 是新的显式动作，不是重启后悄悄重发旧请求。

## 什么时候需要用户再次操作

| 结果 | 界面应提供什么 |
| --- | --- |
| `interactionRequired` 或任务 `needsUserAction` | 一个明确的“继续”操作，由用户点击后交给宿主审批 |
| `vaultMissing`、`vaultLocked`、`vaultRequiresReload` | 对应状态说明与宿主处理入口；插件不收集 Vault 密码 |
| `revoked`、资源关闭、包被替换 | 结束当前操作，丢弃旧 handle；不要后台重开资源 |
| `outcomeUnknown` | 显示结果未确认，让用户核对实际状态后决定是否重试 |

定时器、后台任务步骤、`onOpen` 和恢复回调都不能充当用户审批。先返回当前状态，等待用户明确点击；这会让插件在拒绝、撤销和重启时仍然可理解。完整错误处理见[错误与排查](./errors.zh-CN.md)。
