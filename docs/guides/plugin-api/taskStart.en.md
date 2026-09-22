# taskStart

Start a packaged workflow and obtain its trackable task snapshot.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::TaskStart { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "taskStart" | Yes | Discriminator; use the exact listed value |
| `workflowId` | String | Yes | ID declared in the package workflow catalog |
| `inputJson` | Option&lt;String&gt; | No | Workflow-defined JSON encoded as a string, retained in-process only |
| `fileScopeHandles` | Vec&lt;String&gt; | Yes | Preselected file root handles; defaults to an empty array |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "task" | Yes | Discriminator; use the exact listed value |
| `snapshot` | [PluginWorkflowTaskSnapshot](./types.en.md#pluginworkflowtasksnapshot) | Yes | Task snapshot containing task, steps, and optional result |

## Permissions, timing, and resource scope

`describe` assigns no separate capability hint to this method; instance identity, ownership, and invocation context still apply.

Obtain taskId and revision from a task snapshot. taskStart workflowId must exist in the package workflow catalog; the example uses Workflow Demo inspect-delay-inspect; use your own declaration in your plugin. Start and resume require explicit user actions; cancellation still checks ownership and revision. inputJson, file-handle mappings, and results remain in-process; unfinished tasks become interrupted on restart without payload replay. taskResume is a new action, not a guarantee that unknown earlier side effects can be retried safely.

## Example call

```json
{
  "callId": "example.taskStart",
  "operation": {
    "kind": "taskStart",
    "workflowId": "inspect-delay-inspect",
    "fileScopeHandles": []
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="task"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).


## Related methods

[taskGet](./taskGet.en.md) · [taskList](./taskList.en.md) · [taskCancel](./taskCancel.en.md) · [taskResume](./taskResume.en.md)
