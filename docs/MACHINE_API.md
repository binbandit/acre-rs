# Machine API

The machine API is designed for editors, agents, terminal tools, and automation. Human diagnostics never appear on stdout in JSON mode.

## Acquire

```bash
acre --json acquire feature/refunds \
  --holder codex:session-218 \
  --pid 48122
```

Success fields:

```json
{
  "ok": true,
  "path": "/absolute/path",
  "branch": "feature/refunds",
  "target": "feature/refunds",
  "target_kind": "local-branch",
  "lease_id": "opaque-random-id",
  "ownership": "acre",
  "environment": {
    "fingerprint": "sha256",
    "state": "ready",
    "cacheRoots": ["node_modules"],
    "requiredRoots": ["node_modules"],
    "presentRoots": ["node_modules"],
    "cloneMode": "reuse"
  },
  "reused": true,
  "elapsed_ms": 193
}
```

Every acquisition receives its own opaque lease id, and every lease must be released. Acquiring an already-open workspace does not move or reset it.

## Create and acquire

```bash
acre --json acquire feature/new-work \
  --new \
  --from origin/main \
  --holder editor:zed
```

Unknown targets without `--new` fail and never create a branch.

## Release

```bash
acre --json release --lease-id <id>
```

Release always releases that exact lease. When it was the final lease, Acre attempts safe return. A retained workspace returns `ok: true`, `retained: true`, and the full safety assessment because lease release itself succeeded.

```bash
acre --json release --lease-id <id> --keep-active
```

Releases only the lease.

## Errors

```json
{
  "ok": false,
  "error": {
    "code": "ACRE_TARGET_NOT_FOUND",
    "message": "No existing work is named feature/refudns.",
    "details": {
      "selector": "feature/refudns",
      "suggestions": ["feature/refunds"]
    }
  }
}
```

Exit classes:

```text
0    success
1    internal error
2    usage/configuration
3    environment unavailable
4    target not found
5    concurrent/conflicting state
6    safe refusal/retained workspace
7    Git failure
8    network/provider failure
130  interrupted
194  private shell-resume protocol only
```
