# credential

Create, list, or revoke plugin-owned credential references without exposing secrets.

## Calling the method

Use SDK `api_request(request_id, call_id, PluginApiOperation::Credential { … })` and return the output for the current host request. See [calling the API](../developers/development/calling-api.en.md) for framing and `BrokerResult` parsing. Fields below belong inside `operation`; the outer call requires a string `callId`.

## Request parameters

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "credential" | Yes | Discriminator; use the exact listed value |
| `operation` | [PluginCredentialOperation](./types.en.md#plugincredentialoperation) | Yes | Nested discriminated union; select fields by kind |

## Return value

On success, `outcome.kind="completed"`; the table describes `outcome.value`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | "credential" | Yes | Discriminator; use the exact listed value |
| `result` | [PluginCredentialResult](./types.en.md#plugincredentialresult) | Yes | Nested result union; inspect result.kind first |

## Permissions, timing, and resource scope

Capability hint: `credentialsPlugin`. A declaration still requires an effective grant and any applicable exact-operation approval.

create carries only label, origin, injection mode, and idempotency metadata; Core receives the secret through a protected surface. list exposes handle/revision/state, never secret bytes. Only ready references can be used as networkStart credentials, with matching origin.

## Example call

```json
{
  "callId": "example.credential",
  "operation": {
    "kind": "credential",
    "operation": {
      "kind": "list"
    }
  }
}
```

## Handling results and failures

Correlate the reply by `callId`, then match `outcome.kind` and `value.kind="credential"` before reading the fields above.

Read the stable failure `code`: correct fields, formats, or bounds for `invalidRequest`; stop using stale grants/handles for `permissionDenied`/`revoked`; when protected interaction is needed in a background context, handle `interactionRequired` by waiting for an explicit user action. Never automatically replay writes with unknown outcomes. See [error handling](../developers/development/errors.en.md).

## Sub-operations

The inner `operation.kind` selects a branch below. Full fields and result objects are in the [type reference](./types.en.md#plugincredentialoperation).

| kind | Parameters (? means optional) | result.kind |
| --- | --- | --- |
| `create` | `operationId`, `idempotencyKey`, `label`, `target` | `created` |
| `list` | — | `list` |
| `revoke` | `handle`, `expectedRevision` | `revoked` |

Each example below is a standalone PluginApiCall. Replace every handle, fingerprint, precondition, and revision with preceding query results; these examples are not directly executable.

### create

```json
{
  "callId": "example.credential.create",
  "operation": {
    "kind": "credential",
    "operation": {
      "kind": "create",
      "operationId": "credential-create",
      "idempotencyKey": "credential-create-1",
      "label": "Example service",
      "target": {
        "origin": "https://example.test",
        "injection": {
          "kind": "bearer"
        }
      }
    }
  }
}
```

### list

```json
{
  "callId": "example.credential.list",
  "operation": {
    "kind": "credential",
    "operation": {
      "kind": "list"
    }
  }
}
```

### revoke

```json
{
  "callId": "example.credential.revoke",
  "operation": {
    "kind": "credential",
    "operation": {
      "kind": "revoke",
      "handle": "019d0000-0000-7000-8000-000000000001",
      "expectedRevision": "1"
    }
  }
}
```


## Related methods

[networkStart](./networkStart.en.md) · [permissions](./permissions.en.md)
