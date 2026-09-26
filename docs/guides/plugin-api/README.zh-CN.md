# Broker API 参考

协议 **1.13** 的全部 **44** 个方法。每篇包括请求与返回表、前置条件和可反序列化的 PluginApiCall JSON。先看[调用 API](../developers/development/calling-api.zh-CN.md)，再按需要查下面的方法。

## 原生能力与调用上下文

首先调用 describe 获取当前方法与限额。available 仅表示方法已实现，不表示授权、原生 UI、串口设备或目标系统已经就绪。serialDevices 的候选可能变化；serialOpen 需要真实设备与 Core 保护确认，不能由浏览器页面模拟授权。protocolOpen 的启动记录还需宿主接收并打开终端。

资源均绑定当前插件包、授权、调用所有者和实例 generation；保留句柄直到关闭，不跨实例复用。timer、工作流和 Provider 回调属于后台上下文，不能弹出受保护请求。工作流仅能使用允许的资源方法，不能注册应用 UI、请求文件选择、访问凭据管理、请求输入或启动嵌套任务；Provider 仅允许 describe、resourcesList、resourceClose、networkStart/networkSend 与 serialDevices/serialOpen/serialSend。

`permissionRequest` 当前仅支持 declarative action，Wasm isolated 调用返回 `unsupported`；`terminalRequestInput` 的 isolated 调用返回 `interactionRequired`。二者的 wire 类型存在不代表 Wasm 可以执行受保护交互。

## 发现与权限

| 方法 | 用途 | 成功 value.kind |
| --- | --- | --- |
| [describe](./describe.zh-CN.md) | 查询当前 Broker 方法、能力提示和运行时限额。 | `description` |
| [permissions](./permissions.zh-CN.md) | 读取当前能力授权和可管理的记住操作授权摘要。 | `permissions` |
| [permissionRequest](./permissionRequest.zh-CN.md) | 由用户明确操作发起一次能力授权请求。 | `permissionRequested` |
| [permissionRevoke](./permissionRevoke.zh-CN.md) | 撤销当前插件拥有的一项记住操作授权。 | `permissionRevoked` |
| [permissionsForget](./permissionsForget.zh-CN.md) | 清除当前插件仍拥有的全部记住操作授权。 | `permissionsForgotten` |

## 应用集成

| 方法 | 用途 | 成功 value.kind |
| --- | --- | --- |
| [appRegister](./appRegister.zh-CN.md) | 注册命令、可选快捷键与状态文字。 | `appAccepted` |
| [appNotify](./appNotify.zh-CN.md) | 在宿主显示一条插件通知。 | `appAccepted` |
| [appNavigate](./appNavigate.zh-CN.md) | 响应用户操作，导航到允许的应用路由或自身插件页面。 | `appAccepted` |

## 任务与工作流

| 方法 | 用途 | 成功 value.kind |
| --- | --- | --- |
| [taskStart](./taskStart.zh-CN.md) | 启动包中声明的工作流，并取得可跟踪的任务快照。 | `task` |
| [taskGet](./taskGet.zh-CN.md) | 读取自身任务的最新状态、步骤和进程内结果。 | `task` |
| [taskList](./taskList.zh-CN.md) | 列出当前插件拥有的任务快照。 | `tasks` |
| [taskCancel](./taskCancel.zh-CN.md) | 使用最新 revision 请求取消任务及其资源。 | `task` |
| [taskResume](./taskResume.zh-CN.md) | 通过新的显式用户动作恢复可恢复任务。 | `task` |

## 资源与事件

| 方法 | 用途 | 成功 value.kind |
| --- | --- | --- |
| [resourcesList](./resourcesList.zh-CN.md) | 枚举当前调用所有者范围内的资源。 | `resources` |
| [resourceEvents](./resourceEvents.zh-CN.md) | 分批取出资源产生的有界事件。 | `resourceEvents` |
| [resourceClose](./resourceClose.zh-CN.md) | 关闭自身资源并触发相应清理。 | `closed` |
| [timerStart](./timerStart.zh-CN.md) | 创建一次性或周期定时器。 | `timerStarted` |
| [subscriptionStart](./subscriptionStart.zh-CN.md) | 订阅允许范围内的元数据变更。 | `subscriptionStarted` |

## 分类数据

Core API 1.89 声明了分类数据方法。`dataCatalog`、`dataRead` 和受保护数据操作已在 Core service 接线；原生及端到端同步尚待验收。自建同步插件只选 hosts、credentials、desktopProfiles。

| 方法 | 结果 kind |
| --- | --- |
| [dataCatalog](./dataCatalog.zh-CN.md) | `dataCatalog` |
| [dataRead](./dataRead.zh-CN.md) | `dataRead` |
| [dataSnapshot](./dataSnapshot.zh-CN.md) | `dataSnapshot` |
| [dataInspect](./dataInspect.zh-CN.md) | `dataInspect` |
| [dataCompose](./dataCompose.zh-CN.md) | `dataCompose` |
| [dataReview](./dataReview.zh-CN.md) | `dataReview` |
| [dataApply](./dataApply.zh-CN.md) | `dataApply` |
| [dataExport](./dataExport.zh-CN.md) | `dataExport` |
| [dataCheckpoint](./dataCheckpoint.zh-CN.md) | `dataCheckpoint` |
| [dataRelease](./dataRelease.zh-CN.md) | `dataRelease` |

## 网络

| 方法 | 用途 | 成功 value.kind |
| --- | --- | --- |
| [networkStart](./networkStart.zh-CN.md) | 为确定的端点创建 HTTP、WebSocket、TCP、UDP 或 TLS 资源。 | `networkStarted` |
| [networkSend](./networkSend.zh-CN.md) | 向已批准的网络资源写入数据。 | `networkSent` |

## 文件与 SFTP

| 方法 | 用途 | 成功 value.kind |
| --- | --- | --- |
| [filePick](./filePick.zh-CN.md) | 打开原生选择器并申请精确的本地文件访问范围。 | `filePicked` |
| [file](./file.zh-CN.md) | 在已选择的本地文件范围内读写、列举和监听。 | `file` |
| [sftpOpen](./sftpOpen.zh-CN.md) | 为已授权主机请求独立的 SFTP 根目录资源。 | `sftp` |
| [sftp](./sftp.zh-CN.md) | 通过根目录和条目句柄操作远程文件。 | `sftp` |

## 存储与凭据

| 方法 | 用途 | 成功 value.kind |
| --- | --- | --- |
| [storage](./storage.zh-CN.md) | 读写插件私有的非秘密 KV、Blob、缓存和 schema 状态。 | `storage` |
| [credential](./credential.zh-CN.md) | 创建、列出或撤销插件自有的非明文凭据引用。 | `credential` |

## 进程与远程执行

| 方法 | 用途 | 成功 value.kind |
| --- | --- | --- |
| [processStart](./processStart.zh-CN.md) | 请求执行一个绝对路径程序和确定的参数列表。 | `processStarted` |
| [processSend](./processSend.zh-CN.md) | 向已批准本地进程的 stdin 写入数据或 EOF。 | `processSent` |
| [remoteExecStart](./remoteExecStart.zh-CN.md) | 在已授权主机范围内创建独立 SSH 命令执行资源。 | `remoteExecStarted` |
| [remoteExecSend](./remoteExecSend.zh-CN.md) | 写入远程命令 stdin 或显式发送 EOF。 | `remoteExecSent` |

## 串口与协议

| 方法 | 用途 | 成功 value.kind |
| --- | --- | --- |
| [serialDevices](./serialDevices.zh-CN.md) | 列出可供用户选择的串口候选。 | `serialDevices` |
| [serialOpen](./serialOpen.zh-CN.md) | 为选定串口与串口参数申请受保护授权并创建资源。 | `serialStarted` |
| [serialSend](./serialSend.zh-CN.md) | 向自身已打开的串口资源发送二进制数据。 | `serialSent` |
| [protocolOpen](./protocolOpen.zh-CN.md) | 请求宿主为包内协议 Provider 创建终端启动记录。 | `protocolLaunched` |

## 终端输入

| 方法 | 用途 | 成功 value.kind |
| --- | --- | --- |
| [terminalRequestInput](./terminalRequestInput.zh-CN.md) | 请求向已有终端输入文字，写入前由 Core 复核焦点和授权。 | `inputApprovalRequested` / `inputSent` |

[数据类型](./types.zh-CN.md) · [错误处理](../developers/development/errors.zh-CN.md)
