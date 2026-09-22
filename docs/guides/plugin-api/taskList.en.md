# taskList

List task snapshots owned by the current plugin.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::TaskList { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "taskList" | Yes | Discriminator; use the exact listed value |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "tasks" | Yes | Discriminator; use the exact listed value |
| `snapshots` | Vec&lt;[PluginWorkflowTaskSnapshot](./types.en.md#pluginworkflowtasksnapshot)&gt; | Yes | Task snapshots visible to the current plugin |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

Returns snapshots owned by the current plugin; no plugin-identity filter is accepted. Durable summaries remain visible after restart, but unfinished tasks become interrupted and in-process result data is not restored.

## Example call

```json
{
  "callId": "example.taskList",
  "operation": {
    "kind": "taskList"
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="tasks"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[taskStart](./taskStart.en.md) · [taskGet](./taskGet.en.md) · [taskCancel](./taskCancel.en.md) · [taskResume](./taskResume.en.md)
