# Contributing

Keep Acre small, deterministic, and conservative.

Before opening a change:

```bash
./scripts/check.sh
```

Rules:

- Do not invoke Git outside `src/git`.
- Do not print from lifecycle modules.
- Do not add a general `--force` flag.
- Do not run repository-controlled code automatically.
- Do not mutate worktrees Acre does not own.
- Retain state when safety cannot be proven.
- Prefer direct structs and functions over generic service abstractions.
