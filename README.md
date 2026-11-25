# BugSplat CLI

`bugsplat` is a command line for [BugSplat](https://www.bugsplat.com) crash reporting, for people and coding agents. It reads crash reports and crash groups, finds the symbol files a crash needs on your disk, and uploads them.

**The CLI is in development and has not been released.** There are no prebuilt binaries or packages yet, and some commands are not built. The plan is in [issue 1](https://github.com/BugSplat-Git/bugsplat-cli/issues/1).

## Install

For now, you build the CLI from source. You need Rust, installed with [rustup](https://rustup.rs). The first build downloads the toolchain version pinned in `rust-toolchain.toml`.

```bash
git clone https://github.com/BugSplat-Git/bugsplat-cli.git
cd bugsplat-cli
cargo install --path crates/bugsplat-cli --locked
bugsplat --version
```

`cargo install` puts the `bugsplat` binary in `~/.cargo/bin`. `bugsplat --version` prints the version and the target the binary was built for, for example `bugsplat 0.1.0 (aarch64-apple-darwin)`.

The first release will be the npm package `@bugsplat/cli`, for macOS, Linux and Windows on x64 and arm64. The package downloads the binary for your platform when you install it, and the `bugsplat` command it adds starts that binary through Node.js. You will need Node.js 14.18 or later to install the package and to run `bugsplat`. Each release will also have the binary itself for each platform, which runs without Node.js.

## Sign in

```bash
bugsplat login
```

`bugsplat login` opens your browser so you can sign in to BugSplat. With `--no-browser`, it prints the sign-in URL and leaves opening it to you.

The CLI stores your session in your operating system's keychain. On a machine without a keychain, such as a container, it stores the session in a file that only your user can read. Set `BUGSPLAT_TOKEN_STORE=file` to always use the file. Builds made with `cargo build` (debug builds) use the file by default, so development never touches your keychain.

`bugsplat whoami` shows who is signed in. `bugsplat logout` deletes the stored session.

## Commands

The table lists the commands in the first release.

| Command | What it does |
| --- | --- |
| `bugsplat login`, `logout`, `whoami` | Sign in through your browser, sign out, and show who is signed in |
| `bugsplat databases list` | List the databases you can open |
| `bugsplat reports list`, `reports get <id>`, `reports reprocess <id>` | List crash reports, show one, or ask BugSplat to process one again |
| `bugsplat groups list`, `groups get <id>`, `groups set-status <id> <status>` | List crash groups, show one, or change its status |
| `bugsplat versions list` | List application versions |
| `bugsplat defects create`, `link`, `unlink` | File a crash group as a defect in your issue tracker, link it to a defect that already exists, or remove the link |
| `bugsplat tool list`, `tool call <name>` | List the tools of BugSplat's MCP server, or call one by name |
| `bugsplat symbols list` | List the symbol files uploaded to a database |
| `bugsplat symbols info <file>...` | Print each file's debug id, module name, and type |
| `bugsplat symbols find` | Search folders for symbol files that match debug ids or names |
| `bugsplat symbols upload`, `symbols delete` | Upload an application version's symbol files, or delete them |
| `bugsplat symbols dump <file>...` | Generate Breakpad `.sym` files from binaries, without uploading them |
| `bugsplat completions <shell>` | Print a shell completion script |

The commands that read crash data call the tools of BugSplat's hosted MCP server, so `bugsplat groups list` returns what a coding agent gets from the `list-groups` tool.

These options work with every command:

| Option | Environment variable | What it does |
| --- | --- | --- |
| `--database`, `-b` | `BUGSPLAT_DATABASE` | The BugSplat database to use |
| `--api-url` | `BUGSPLAT_API_URL`, `BUGSPLAT_HOST` | Base URL of the BugSplat API |
| `--json` | | Print JSON for scripts. The default output is plain text. For the commands that read crash data, `--json` asks the server's tool for `format: "json"` and prints the document it returns. |
| `--quiet` | | Print only results and errors |
| `--verbose` | | Print more detail about what the command is doing |

Results go to stdout and errors go to stderr. The exit code is 0 for success, 1 for a failure, 2 for a usage error, and 3 when you are not signed in.

## Use in CI

A build machine has no browser, so it authenticates with an OAuth Client ID and Client Secret. Create the pair as described in [OAuth2 Client Credentials](https://docs.bugsplat.com/introduction/development/web-services/oauth2#client-credentials). Each pair belongs to one database.

Set the credentials as environment variables, or pass `--client-id` and `--client-secret`:

| Environment variable | Value |
| --- | --- |
| `BUGSPLAT_DATABASE` | Your database |
| `BUGSPLAT_CLIENT_ID` | The Client ID |
| `BUGSPLAT_CLIENT_SECRET` | The Client Secret |

When a Client ID and Client Secret are set, the CLI uses them and ignores any stored session.

On Linux, `bugsplat` verifies HTTPS connections with the CA certificates installed on the system. A minimal container image, such as `node:22-slim`, has none, and commands fail with `No CA certificates were loaded from the system`. Install the image's `ca-certificates` package first.

This GitHub Actions step uploads the Windows symbols of a build. It assumes an earlier step put `bugsplat` on the `PATH`.

```yaml
- name: Upload symbols to BugSplat
  env:
    BUGSPLAT_DATABASE: ${{ secrets.BUGSPLAT_DATABASE }}
    BUGSPLAT_CLIENT_ID: ${{ secrets.BUGSPLAT_CLIENT_ID }}
    BUGSPLAT_CLIENT_SECRET: ${{ secrets.BUGSPLAT_CLIENT_SECRET }}
  run: bugsplat symbols upload -a MyApp -v 1.0.0 -d build/Release -f "**/*.{pdb,exe,dll}"
```

## Coming from symbol-upload

`bugsplat symbols upload` does what the `symbol-upload` tool does and keeps its short flags, so most commands translate by changing the first words:

```bash
# before
npx @bugsplat/symbol-upload -b fred -a MyApp -v 1.0.0 -d build/Release -f "**/*.{pdb,exe,dll}"

# after
bugsplat symbols upload -b fred -a MyApp -v 1.0.0 -d build/Release -f "**/*.{pdb,exe,dll}"
```

| `symbol-upload` | `bugsplat symbols upload` |
| --- | --- |
| `-b`, `--database` | `-b`, `--database`. `BUGSPLAT_DATABASE` still works. |
| `-a`, `--application` | `-a`, `--application` |
| `-v`, `--version` | `-v`, `--version` |
| `-f`, `--files` | `-f`, `--files`. The default is still `*.js.map`. |
| `-d`, `--directory` | `-d`, `--directory`. The default is still the current directory. |
| `-w`, `--workers` | `-w`, `--workers` |
| `-l`, `--localPath` | `-l`, `--local-path` |
| `-i`, `--clientId` | `--client-id`, or `BUGSPLAT_CLIENT_ID`. `SYMBOL_UPLOAD_CLIENT_ID` still works. |
| `-s`, `--clientSecret` | `--client-secret`, or `BUGSPLAT_CLIENT_SECRET`. `SYMBOL_UPLOAD_CLIENT_SECRET` still works. |
| `-r`, `--remove` | A separate command: `bugsplat symbols delete -b fred -a MyApp -v 1.0.0` |
| `-m`, `--dumpSyms` | `-m`, `--dump-syms`. Generates a Breakpad `.sym` file from each selected binary and uploads that in place of the binary. |
| `-h`, `--help` | `-h`, `--help` |

What changes:

- Long options with two words are written with a hyphen: `--local-path`, `--client-id`, `--client-secret`.
- The Client ID and Client Secret have no short flags. Use the long options or the environment variables.
- Like `symbol-upload`, `bugsplat symbols upload` reads the application name and version from `./package.json` when you leave out `-a` and `-v`. `bugsplat symbols delete` requires both.
- `bugsplat symbols upload` uploads every file that has a debug id, so `.so`, `.dylib` and extensionless binaries matched by your `-f` pattern upload too. `symbol-upload` skips them.
- `bugsplat symbols delete` asks you to confirm on a terminal. In a script, add `--yes`.

`bugsplat symbols upload --dry-run` lists the files it would upload, with each file's debug id, and uploads nothing.

## Development

```bash
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

CI runs the same checks on Linux, and the tests on macOS and Windows, for every pull request. CI also runs [`cargo deny`](https://github.com/EmbarkStudios/cargo-deny) to check dependency licenses and security advisories. Run `cargo deny check` yourself after you add a dependency.

Releases are built with [dist](https://github.com/axodotdev/cargo-dist) from `dist-workspace.toml`. `dist plan` shows what a release would build. The release workflow runs only when a `v*` version tag is pushed, and nothing is published until the repository is public. To build and smoke-test every target without a release, start the "Build release artifacts" workflow by hand. [RELEASING.md](RELEASING.md) has the release checklist.

## License

MIT
