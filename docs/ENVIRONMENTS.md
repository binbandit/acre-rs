# Environment reuse

Acre treats a Git checkout and a prepared development environment as separate things.

Git can switch tracked files to any commit. Acre decides whether ignored dependency and build data may be reused by comparing an exact **environment generation**.

## Fingerprints

A generation fingerprint contains:

- hashes of detected lockfiles and toolchain declarations at the target commit;
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

## Copy strategy

Acre prefers whole-slot reuse. When a second compatible workspace needs the same environment while the first remains active, Acre attempts:

1. native directory block clone on macOS or Linux;
2. per-file `COPYFILE_FICLONE_FORCE`;
3. a normal recursive copy.

The reported mode is honest. Acre does not label a full copy as a reflink.

## Seed files

Seed files are local configuration, not caches. Defaults are `.env` and `.env.local`.

They are:

- copied only when the source path is Git-ignored and the target is trusted;
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

This means the cold cost is paid once per generation, not once per branch.
