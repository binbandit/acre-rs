# Output gallery

This document uses semantic tags to describe intended terminal styling. The implementation renders these as ANSI when stdout is a TTY and strips them otherwise.

## Warm creation

```text
<bold><green>Created</green></bold> <blue>feature/refunds</blue>
<dim>from origin/main</dim>

<green>→</green> <bold><blue>feature/refunds</blue></bold>
<dim>workspace ready · reused environment 52ba538f · 284 ms</dim>
```

## Existing local branch

```text
<green>→</green> <bold><blue>release/2026.08</blue></bold>
<dim>workspace ready · environment 52ba538f · 193 ms</dim>
```

## Remote branch

```text
<dim>Tracking</dim> <blue>origin/alex/refund-fix</blue>
<green>→</green> <bold><blue>alex/refund-fix</blue></bold>
<dim>workspace ready · reused environment 52ba538f · 241 ms</dim>
```

## Fork PR

```text
<dim>Opening PR #1842</dim>
<green>→</green> <bold><blue>PR #1842 · Fix checkout-state race</blue></bold>
<dim>workspace ready · reused environment 52ba538f · 412 ms</dim>
<yellow>Fork secrets withheld · no repository code was run</yellow>
```

## Cold generation

```text
<green>→</green> <bold><blue>feature/upgrade-react</blue></bold>
<dim>code ready · environment 7ca9ce41 cold · 227 ms</dim>

<yellow>No compatible prepared environment exists yet.</yellow>
<dim>Run the project’s normal install or build command. Acre will retain the clean result for matching branches.</dim>
```

## Safe done

```text
<bold><green>Finished with</green></bold> <blue>feature/refunds</blue>

<dim>Branch and commits preserved</dim>
<dim>Warm workspace returned to the pool</dim>
```

## Refused done

```text
<bold><yellow>feature/refunds is still in use</yellow></bold>

  2 modified
  1 untracked
  changed local file: .env.local
  held by editor:zed

<dim>Acre left the workspace exactly where it is.</dim>
```

## Unknown target

```text
<bold><red>No existing work is named feature/refudns.</red></bold>

Closest matches:
  <blue>feature/refunds</blue>
  <blue>feature/refund-history</blue>

Start new work explicitly:
  <blue>acre new feature/refudns</blue>
```
