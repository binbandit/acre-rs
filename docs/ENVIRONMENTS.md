# Environment reuse

Acre treats a Git checkout and a prepared development environment as separate things.

Git can switch tracked files to any commit. Acre decides whether ignored dependency and build data may be reused by comparing an exact **environment generation**.

## Fingerprints

A generation fingerprint contains:

- hashes of recognized manifests, lockfiles, package-manager settings, and toolchain declarations at the target commit, including nested workspace packages;
- the contents of each Node package's `patches/` directory, which patch-package applies during install;
- the detected ecosystem set;
- resolved cache and required roots;
- operating system, architecture, OS major version, and, for Node projects, the Node major version.

The OS version is read without a `PATH` lookup. Node is asked only for Node projects, and from the primary checkout, so version managers choose the same `node` whatever directory the caller runs in. A caller whose `PATH` has no `node` at all, such as a GUI editor using the machine API, gets a different generation than a terminal that has one. Acre accepts that cold start rather than guess, because presenting native modules built for another Node as compatible would break the workspace. Give such callers the same `PATH` as your shell to share warm slots.

Ecosystems are detected in nested projects as well as at the repository root. Package identity (`name`, `version`), executable links (`bin`), dependency declarations, and installation lifecycle scripts affect package fingerprints. Ordinary test-script edits do not.

Acre does not claim two workspaces are compatible merely because they use the same branch family or package manager.

## States

- **ready**: every required root exists;
- **warm**: optional caches exist but at least one required root is absent;
- **cold**: no approved reusable root exists;
- **unknown**: recovered metadata could not be verified.

Required roots come from the top-most projects only, at their own directory. A Node app with a helper tool's `tools/lint/pyproject.toml` is ready once `node_modules` exists; the helper's `.venv` stays an optional cache. When the top-most projects are siblings, such as `web/package.json` and `api/pyproject.toml`, each one's root is required (`web/node_modules` and `api/.venv`).

For Node projects, `node_modules` is normally required. Rust’s `target` directory is optional reusable data: without it the generation is reported as `cold`, and with it as `warm`, while the checkout itself remains usable in either case.

## Built-in ecosystem policy

| Ecosystem | Fingerprint files | Cache roots | Required roots | Build output |
|---|---|---|---|---|
| pnpm | `package.json`, `pnpm-lock.yaml`, `pnpm-workspace.yaml`, `.npmrc`, `.pnpmfile.cjs` | `node_modules` | `node_modules` | `dist`, `coverage`, `*.tsbuildinfo` |
| Yarn | `package.json`, `yarn.lock`, `.yarnrc`, `.yarnrc.yml` | `node_modules`, `.yarn/cache`, `.yarn/unplugged`, `.yarn/install-state.gz`, `.yarn/build-state.yml`, `.pnp.cjs`, `.pnp.loader.mjs`, `.pnp.data.json` | `node_modules` | `dist`, `coverage`, `*.tsbuildinfo` |
| npm | `package.json`, `package-lock.json`, `npm-shrinkwrap.json`, `.npmrc` | `node_modules` | `node_modules` | `dist`, `coverage`, `*.tsbuildinfo` |
| Bun | `package.json`, `bun.lock`, `bun.lockb`, `bunfig.toml` | `node_modules` | `node_modules` | `dist`, `coverage`, `*.tsbuildinfo` |
| Turborepo | `package.json`, `turbo.json` | `.turbo` | none | none |
| Next.js | `package.json`, `next.config.*` | `.next/cache` | none | `.next`, `next-env.d.ts`, `out` |
| Rust | `Cargo.toml`, `Cargo.lock`, toolchain files | `target` | none | none |
| Python | `pyproject.toml`, `uv.lock`, Poetry, requirements, Pipenv, and setuptools files | `.venv`, `__pycache__`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache` | `.venv` | none |
| Go | `go.mod`, `go.sum`, `go.work`, `go.work.sum` | none by default | none | none |

Every repository also treats `.DS_Store` and `Thumbs.db`, which file managers leave behind, as build output.

Built-ins can be extended in user config or `.acre.json` with `cacheRoots`, `requiredRoots`, `outputRoots`, and `seedFiles`, and removed with `excludedRoots`. Paths are data only and must remain repository-relative.

A cache root applies at any depth, so a monorepo package's `packages/app/node_modules` is cloned, cleared, and excluded from the ignored-data check exactly like the top-level `node_modules`. Only Git-ignored directories qualify, plus the install-state files listed above, including at the top level. Tracked paths and symlinked cache roots are not copied or cleared.

When a slot is recycled for a different generation, its old cache roots are renamed into the repository's `trash` directory and a detached Acre process deletes them, so opening the workspace does not wait for tens of thousands of files to be unlinked. `acre system gc` empties anything a background purge left behind.

## Build output

Build output is ignored data a tool regenerates from the code, such as Next.js's `.next` or TypeScript's `tsconfig.tsbuildinfo`. It never blocks `done`. When a workspace returns to the pool, Acre removes it so the next branch never sees another branch's server bundles or generated route types, and keeps any cache root inside it: `.next` goes, `.next/cache` stays. Build output is never cloned between workspaces and is not part of the fingerprint.

Like cache roots, output roots match at any depth. A component starting with `*` matches any longer name with that ending, so `*.tsbuildinfo` matches `packages/ui/tsconfig.build.tsbuildinfo`. Only real files and directories qualify; a symlink stays unknown data.

Declare other build output in `.acre.json`:

```json
{ "environment": { "outputRoots": ["storybook-static", "playwright-report"] } }
```

Declare only paths a build fully regenerates. Data a tool keeps between runs, such as a local database, is not build output; keep it out of the workspace or let `done` refuse.

## Copy strategy

Acre prefers whole-slot reuse. When a second compatible workspace needs the same environment while the first remains active, Acre attempts:

1. a filesystem clone: on macOS one `clonefile(2)` call clones each cache root's whole tree inside the kernel, and on Linux `cp -a --reflink=always` clones it file by file;
2. a normal recursive copy when cloning is unavailable.

Copies are staged in a temporary directory and published only after completion. Before copying and again before publication, Acre checks the source checkout's current commit and refuses sources with staged, unstaged, or untracked fingerprint files. Ordinary source-code edits do not prevent cache sharing.

The reported mode is honest. Acre does not label a full copy as a reflink: `clonefile(2)` and `--reflink=always` fail instead of copying when the paths are on different volumes or the filesystem cannot clone, and Acre then copies and counts the files itself.

## Seed files

Seed files are local configuration, not caches. Defaults are `.env` and `.env.local`.

They are:

- copied only when both source and destination paths are Git-ignored, the destination is absent, and the target is trusted;
- hashed after activation;
- blocked from return if modified;
- removed before the workspace returns to the pool, along with any ignored seed-named file created in the workspace;
- never copied into cross-repository pull requests;
- never retained in an untrusted workspace generation.

Paths are normalized by component, including repeated separators and `.` components, before checking seed boundaries. Seed paths and cache roots must not overlap; Acre rejects conflicting configuration before copying. Expanded cache paths inside directory seeds are withheld from copying and slot reuse, including caches already present in older slots. Ignored files beside a nested seed are checked individually.

A directory may be configured as a seed path. Acre hashes its contents recursively and refuses `done` when any part changes.

A seed that is a symlink stays a symlink. A relative link that still resolves from the workspace, such as `.env -> config/dev.env` into the checkout's own tracked file, is copied as is. One that would dangle there, such as `.env -> ../shared/app.env`, is pointed at the file it reached from the primary checkout.

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
