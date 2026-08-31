# Command reference

## Global options

```text
-C, --directory <path>  run as though started in another directory
--json                  emit one stable JSON document
--no-color              disable ANSI colours
--plain                 use ASCII-only human output
--verbose               retain underlying diagnostics in errors
-V, --version           print version
```

## `acre [target] [-- command...]`

With no target, open the interactive existing-target picker.

Target forms:

```text
feature/refunds
origin/alex/refund-fix
pr:1842
https://github.com/acme/repo/pull/1842
/path/to/an/existing/worktree
-
```

`-` means the previous exact path for the current shell and cannot be combined with a child command.

Anything after a literal `--` is executed in the materialised target. Extra positional arguments without `--` are rejected.

## `acre new <branch>`

```text
--from <ref>  explicit base; `.` means the current HEAD
--fresh       fetch exact remote base before creation
--stay        do not navigate the current shell
```

Fails when the local branch already exists. Never pushes.

## `acre done [target]`

Defaults to the current worktree. The repository primary worktree is permanent. External worktrees are left untouched.

For an Acre-owned workspace, safe return requires:

```text
clean Git status
no operation in progress
no changed seed files
no new unknown ignored data
no other leases
no detectable process use
unlocked and registered worktree
```

The branch and commits are preserved.

## `acre setup`

```text
--shell <bash|zsh|fish|powershell>
-y, --yes
```

Creates config if absent and idempotently installs or upgrades a marked shell block.

## Machine commands

Hidden from normal root help:

```text
acre --json acquire <target> --holder <label> [--pid <pid>]
acre --json acquire <branch> --new --holder <label> [--from <ref>] [--fresh]
acre --json release --lease-id <id> [--keep-active]
```

## System commands

```text
acre system warm [--slots <count>]
acre system inspect
acre system doctor
acre system repair
acre system gc
```

## Configuration commands

```text
acre config show
acre config path
acre config init [--force]
acre config set <dotted-key> <json-or-string>
acre config edit
acre config repo-init [--force]
```

## Reserved names and explicit target types

The words `new`, `done`, `setup`, `system`, and `config` are command names. A branch with one of those exact names remains addressable with an explicit type:

```bash
acre branch:done
acre remote:origin/new
acre worktree:/absolute/path/to/worktree
```

Git branch names cannot contain `:`, so typed target prefixes are unambiguous.
