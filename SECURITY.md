# Security

Acre manipulates Git worktrees and may copy dependency caches. Treat filesystem-boundary, symlink, path-traversal, cache-poisoning, seed-file, fork-PR, and command-injection issues as security-sensitive.

Do not open public issues containing credentials or private repository details. Report suspected vulnerabilities privately to the repository owner.

Acre never intentionally uploads source or environment data and never runs repository-controlled setup commands automatically.
