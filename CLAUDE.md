# BugSplat CLI

One binary, `bugsplat`, for reading crash data, finding the symbol files a crash needs, and uploading them. The plan and the work streams are in https://github.com/BugSplat-Git/bugsplat-cli/issues/1.

## Commands

```bash
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -- --help
```

All four checks must pass before a pull request.

## Layout

- `crates/bugsplat-cli`: the binary. `src/cli.rs` wires the command tree; each command's arguments and `run` function live in its own file under `src/commands/`. Add a command by adding a file, not by growing a shared one.
- `crates/bugsplat-core`: `Config`, `Error` (with exit codes), the HTTP client.
- `crates/bugsplat-auth`: `access_token(&Config)` is the only way any crate gets a bearer token.
- `crates/bugsplat-mcp-client`: client for the hosted MCP server; the remote verbs call its tools.
- `crates/bugsplat-mcp-server`: `bugsplat mcp`, a local MCP server over stdio with the symbol tools the hosted server can't have. Nothing but protocol messages may go to stdout while it runs.
- `crates/bugsplat-debugid`: `read(path)` returns debug ids; they must match BugSplat's npm packages (`pdb-guid`, `macho-uuid`, `elfy`, `symbol-upload`).
- `crates/bugsplat-symbols`: find symbol files on disk. `crates/bugsplat-upload`: upload and delete.

## Rules

- Never print, log or put in an error message a token, Client Secret or password. Types that hold one implement `Debug` by hand.
- Tests never read the developer's real session or keychain: every spawned `bugsplat` gets `BUGSPLAT_TOKEN_STORE=file` and a temporary `BUGSPLAT_CONFIG_DIR`.
- Tests never call production. Unit tests use mock servers (`wiremock`); end-to-end checks use LocalStack at `http://localhost:8085` with `BUGSPLAT_API_URL` set, and a token from `scripts/localstack-token.py` in `BUGSPLAT_ACCESS_TOKEN`.
- Results go to stdout, everything else to stderr. `--json` output is stable; text output is for people.
- Exit codes: 0 success, 1 failure, 2 usage, 3 not signed in (`bugsplat_core::Error::exit_code`).
- Shared dependency versions are in the root `Cargo.toml` under `[workspace.dependencies]`. A dependency only one crate needs goes in that crate's `Cargo.toml`.
- Comments explain why, not what. No section-label or narrating comments.
- Nothing is published (crates.io, Homebrew, npm, GitHub Releases) without Bobby's say-so.
- Never commit or push to `main`; work on a branch and open a pull request.
