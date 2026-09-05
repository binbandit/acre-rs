# Environment reuse

Acre treats a Git checkout and a prepared development environment as separate things.

Git can switch tracked files to any commit. Acre decides whether ignored dependency and build data may be reused by comparing an exact **environment generation**.

## Fingerprints

A generation fingerprint contains:

- hashes of recognized manifests, lockfiles, and toolchain declarations at the target commit, including nested workspace packages;
- the detected ecosystem set;
- resolved cache and required roots;
- operating system, architecture, OS major version, and Node major version.

Acre does not claim two workspaces are compatible merely because they use the same branch family or package manager.

## States

- **ready** — every required root exists;
- **warm** — optional caches exist but at least one required root is absent;
- **cold** — no approved reusable root exists;
- **unknown** — recovered metadata could not be verified.

For Node projects, `node_modules` is normally required. Rust’s `target` directory is optional reusable data: without it the generation is reported as `cold`, and with it as `warm`, while the checkout itself remains usable in either case.

## Built-in ecosystem policy

| Ecosystem | Fingerprint files | Cache roots | Required roots |
|---|---|---|---|
| pnpm | `package.json`, `pnpm-lock.yaml` | `node_modules` | `node_modules` |
| Yarn | `package.json`, `yarn.lock`, `.yarnrc.yml` | `node_modules`, `.yarn/cache`, `.yarn/unplugged` | `node_modules` |
| npm | `package.json`, `package-lock.json`, `npm-shrinkwrap.json` | `node_modules` | `node_modules` |
| Bun | `package.json`, `bun.lock`, `bun.lockb` | `node_modules` | `node_modules` |
| Turborepo | `package.json`, `turbo.json` | `.turbo` | none |
| Next.js | `package.json`, `next.config.*` | `.next/cache` | none |
| Rust | `Cargo.toml`, `Cargo.lock`, toolchain files | `target` | none |
| Python | `pyproject.toml`, `uv.lock`, Poetry/requirements files | `.venv`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache` | `.venv` |
| Go | `go.mod`, `go.sum` | none by default | none |

Built-ins can be extended or excluded in user config or `.acre.json`. Paths are data only and must remain repository-relative.

A cache root applies at any depth, so a monorepo package's `packages/app/node_modules` is cloned, cleared, and excluded from the ignored-data check exactly like the top-level `node_modules`. Only Git-ignored directories qualify, including at the top level. Tracked paths and symlinked cache roots are not copied or cleared.

## Copy strategy

Acre prefers whole-slot reuse. When a second compatible workspace needs the same environment while the first remains active, Acre attempts:

1. a filesystem clone using `cp -cR` on macOS or `cp -a --reflink=always` on Linux;
2. a normal recursive copy when cloning is unavailable.

Copies are staged in a temporary directory and published only after completion. Before copying, Acre checks the source checkout's current commit and refuses sources with staged, unstaged, or untracked fingerprint files. Ordinary source-code edits do not prevent cache sharing.

The reported mode is honest. Acre does not label a full copy as a reflink.

## Seed files

Seed files are local configuration, not caches. Defaults are `.env` and `.env.local`.

They are:

- copied only when both source and destination paths are Git-ignored, the destination is absent, and the target is trusted;
- hashed after activation;
- blocked from return if modified;
- always removed from idle pool slots;
- never copied into cross-repository pull requests;
- never retained in an untrusted workspace generation.

A directory may be configured as a seed path. Acre hashes its contents recursively and refuses `done` when any part changes.

## Cold generations

Acre never runs repository code automatically. When a generation is cold:

1. Acre opens the code immediately;
2. the human or calling tool runs the normal install/build command;
3. once the workspace is clean and safely returned, Acre retains the prepared roots;
4. future matching branches reuse them.

The cold cost is normally paid once per generation per slot, while matching branches reuse the prepared slot.

## Portability

Acre keeps each workspace at a fixed directory through activation, return, and reuse. This preserves installation paths in Python environments and other caches. Directories identify slots; branch names remain visible in Acre but no longer determine the directory name.

Python virtual environments containing `pyvenv.cfg` are reused only in place. Acre skips them when copying caches from another checkout, because their scripts and installed packages may embed absolute paths. Create the environment using the project's normal install command in each concurrent workspace; later matching branches reuse it at that same path. Acre does not run installers or rewrite environment contents automatically. See [Python's venv portability guidance](https://docs.python.org/3/library/venv.html#how-venvs-work).

Existing workspace paths are preserved on upgrade. Environments already copied or moved by an earlier Acre version may need to be recreated once. Other cache roots are copied as configured; applications that embed absolute paths in those caches remain responsible for their portability. A `ready` snapshot verifies required-root presence, not the health of every installed package.
