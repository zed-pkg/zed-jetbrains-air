# Agent instructions

## Product boundary

`zed-air` is a read-only diagnostic bridge for JetBrains Air. Do not move package resolution, installation, publishing, authentication, or store ownership out of `zed-cli`.

## Safety

- Inspection tools must never mutate the workspace.
- Recommended commands must declare whether they are read-only, mutate the project, or install software.
- Never execute a recommended command from inside the MCP server.
- Avoid reading secrets, credential files, or environment-variable values.
- Write logs only to stderr while serving MCP; stdout is reserved for newline-delimited JSON-RPC messages.

## Compatibility

- Keep stable diagnostic codes backward-compatible.
- Negotiate MCP protocol versions and remain compatible with the latest stable version supported by JetBrains Air.
- Treat the 2026 Air native-plugin surface as unavailable until JetBrains publishes an official SDK and compatibility policy.

## Validation

Run:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

## Repository-local Git worktrees

- Create or use a Git worktree only when the human operator explicitly authorizes it for the current task. Concurrency or a dirty checkout is not permission by itself.
- Put every authorized worktree at `<repository-root>/tmp/worktrees/<name>`; from the repository root, use `./tmp/worktrees/<name>`. Never place worktrees beside repositories or organization directories.
- Keep `tmp`, `temp`, `tmp/worktrees`, and `temp/worktrees` ignored in the repository-root `.gitignore`. Do not commit files from those directories.
- Relocate or remove a worktree only when the operator explicitly requests it. Before removal, preserve and publish intended changes, verify its commit is represented on the target branch, and confirm there are no tracked, untracked, ignored-sensitive, or in-use files that must survive. Remove it with `git worktree remove <path>` without `--force`; never delete a worktree directory with `rm`.
