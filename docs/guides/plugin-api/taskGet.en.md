# taskGet

Read the current state, steps, and in-process results of an owned task.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::TaskGet { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "taskGet" | Yes | Discriminator; use the exact listed value |
| `taskId` | [PluginWorkflowTaskId](./types.en.md#pluginworkflowtaskid) | Yes | Opaque task ID from a snapshot |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "task" | Yes | Discriminator; use the exact listed value |
| `snapshot` | [PluginWorkflowTaskSnapshot](./types.en.md#pluginworkflowtasksnapshot) | Yes | Task snapshot containing task, steps, and optional result |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

Take taskId from snapshot.task.taskId returned by taskStart or taskList. Render the actual task.state such as running, needsUserAction, or completed. result is optional; results from a previous process are not restored on restart.

## Example call

UUIDs below are format placeholders. Replace them with the corresponding IDs/handles returned by preceding APIs or Core context. They are not directly executable and cannot be reused across plugins or restarts.

```json
{
  "callId": "example.taskGet",
  "operation": {
    "kind": "taskGet",
    "taskId": "019d0000-0000-7000-8000-000000000001"
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="task"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[taskStart](./taskStart.en.md) · [taskList](./taskList.en.md) · [taskCancel](./taskCancel.en.md) · [taskResume](./taskResume.en.md)
