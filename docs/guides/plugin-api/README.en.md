# Broker API reference

All **44** methods in protocol **1.13**, each with request/result tables, prerequisites, and deserializable PluginApiCall JSON. Start with [calling the API](../developers/development/calling-api.en.md), then select a method below.

## Native availability and invocation context

Call describe first for methods and limits. available means a method is implemented, not that permissions, native UI, serial hardware, or the target system are ready. Serial candidates can change; serialOpen needs a real device and protected Core confirmation, which a browser page cannot simulate. A protocolOpen launch record still needs host delivery and terminal opening.

Resources bind the current package, grants, caller owner, and instance generation. Retain handles until closed, never across instances. Timer, workflow, and provider callbacks are background contexts and cannot open protected prompts. Workflows can use permitted resource methods, but cannot register application UI, pick files, manage credentials, request terminal input, or start nested tasks. Providers allow only describe, resourcesList, resourceClose, networkStart/networkSend, and serialDevices/serialOpen/serialSend.

`permissionRequest` currently supports only declarative actions and returns `unsupported` from Wasm isolated; isolated `terminalRequestInput` returns `interactionRequired`. Having a wire type does not enable protected interaction from Wasm.

## Discovery and permissions

| Method | Purpose | Success value.kind |
| --- | --- | --- |
| [describe](./describe.en.md) | Inspect broker methods, capability hints, and runtime limits. | `description` |
| [permissions](./permissions.en.md) | Read current capability grants and manageable remembered-operation summaries. | `permissions` |
| [permissionRequest](./permissionRequest.en.md) | Request a capability grant from an explicit user action. | `permissionRequested` |
| [permissionRevoke](./permissionRevoke.en.md) | Revoke one remembered operation owned by the current plugin. | `permissionRevoked` |
| [permissionsForget](./permissionsForget.en.md) | Forget all remembered operations still owned by the current plugin. | `permissionsForgotten` |

## App integration

| Method | Purpose | Success value.kind |
| --- | --- | --- |
| [appRegister](./appRegister.en.md) | Register commands, optional shortcuts, and status text. | `appAccepted` |
| [appNotify](./appNotify.en.md) | Show a plugin notification in the host. | `appAccepted` |
| [appNavigate](./appNavigate.en.md) | Navigate to an allowed app route or your plugin page in response to a user action. | `appAccepted` |

## Tasks and workflows

| Method | Purpose | Success value.kind |
| --- | --- | --- |
| [taskStart](./taskStart.en.md) | Start a packaged workflow and obtain its trackable task snapshot. | `task` |
| [taskGet](./taskGet.en.md) | Read the current state, steps, and in-process results of an owned task. | `task` |
| [taskList](./taskList.en.md) | List task snapshots owned by the current plugin. | `tasks` |
| [taskCancel](./taskCancel.en.md) | Request cancellation of a task and its resources using the current revision. | `task` |
| [taskResume](./taskResume.en.md) | Resume an eligible task through a new explicit user action. | `task` |

## Resources and events

| Method | Purpose | Success value.kind |
| --- | --- | --- |
| [resourcesList](./resourcesList.en.md) | Enumerate resources in the current caller ownership scope. | `resources` |
| [resourceEvents](./resourceEvents.en.md) | Drain bounded resource events in batches. | `resourceEvents` |
| [resourceClose](./resourceClose.en.md) | Close an owned resource and trigger its cleanup. | `closed` |
| [timerStart](./timerStart.en.md) | Create a one-shot or repeating timer. | `timerStarted` |
| [subscriptionStart](./subscriptionStart.en.md) | Subscribe to metadata changes within the allowed scope. | `subscriptionStarted` |

## Category data

Core API 1.89 declares category data methods. `dataCatalog`, the separately authorized `dataRead`, and protected exchange operations are wired in Core service; native and end-to-end acceptance remains open. The self-host sync plugin selects only hosts, credentials and desktopProfiles.

| Method | Result kind |
| --- | --- |
| [dataCatalog](./dataCatalog.en.md) | `dataCatalog` |
| [dataRead](./dataRead.en.md) | `dataRead` |
| [dataSnapshot](./dataSnapshot.en.md) | `dataSnapshot` |
| [dataInspect](./dataInspect.en.md) | `dataInspect` |
| [dataCompose](./dataCompose.en.md) | `dataCompose` |
| [dataReview](./dataReview.en.md) | `dataReview` |
| [dataApply](./dataApply.en.md) | `dataApply` |
| [dataExport](./dataExport.en.md) | `dataExport` |
| [dataCheckpoint](./dataCheckpoint.en.md) | `dataCheckpoint` |
| [dataRelease](./dataRelease.en.md) | `dataRelease` |

## Network

| Method | Purpose | Success value.kind |
| --- | --- | --- |
| [networkStart](./networkStart.en.md) | Create an HTTP, WebSocket, TCP, UDP, or TLS resource for one endpoint. | `networkStarted` |
| [networkSend](./networkSend.en.md) | Write bytes to an approved network resource. | `networkSent` |

## Files and SFTP

| Method | Purpose | Success value.kind |
| --- | --- | --- |
| [filePick](./filePick.en.md) | Open a native picker and request an exact local-file access scope. | `filePicked` |
| [file](./file.en.md) | Read, write, list, and watch within a selected local-file scope. | `file` |
| [sftpOpen](./sftpOpen.en.md) | Request an independent SFTP root resource for an authorized host. | `sftp` |
| [sftp](./sftp.en.md) | Operate on remote files using root and entry handles. | `sftp` |

## Storage and credentials

| Method | Purpose | Success value.kind |
| --- | --- | --- |
| [storage](./storage.en.md) | Access plugin-private non-secret KV, blobs, cache, and schema state. | `storage` |
| [credential](./credential.en.md) | Create, list, or revoke plugin-owned credential references without exposing secrets. | `credential` |

## Process and remote execution

| Method | Purpose | Success value.kind |
| --- | --- | --- |
| [processStart](./processStart.en.md) | Request execution of an absolute executable path with a fixed argument list. | `processStarted` |
| [processSend](./processSend.en.md) | Write bytes or EOF to an approved local process stdin. | `processSent` |
| [remoteExecStart](./remoteExecStart.en.md) | Create an independent SSH command resource within an authorized host scope. | `remoteExecStarted` |
| [remoteExecSend](./remoteExecSend.en.md) | Write remote-command stdin or explicitly send EOF. | `remoteExecSent` |

## Serial and protocols

| Method | Purpose | Success value.kind |
| --- | --- | --- |
| [serialDevices](./serialDevices.en.md) | List serial-device candidates for user selection. | `serialDevices` |
| [serialOpen](./serialOpen.en.md) | Request protected approval for a selected serial candidate and settings, then create its resource. | `serialStarted` |
| [serialSend](./serialSend.en.md) | Send binary data to an owned serial resource. | `serialSent` |
| [protocolOpen](./protocolOpen.en.md) | Ask the host to create a terminal launch record for a packaged protocol provider. | `protocolLaunched` |

## Terminal input

| Method | Purpose | Success value.kind |
| --- | --- | --- |
| [terminalRequestInput](./terminalRequestInput.en.md) | Request text input into an existing terminal, with Core rechecking focus and authority before writing. | `inputApprovalRequested` / `inputSent` |

[DTO types](./types.en.md) · [Error handling](../developers/development/errors.en.md)
