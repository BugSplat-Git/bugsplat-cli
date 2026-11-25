# Releasing the BugSplat CLI

Nothing has been released yet. The repository is private, no tag exists, and no package has been published. This page lists what a release does, what has to be in place before the first one, and the steps for each release.

Releases are built by [dist](https://github.com/axodotdev/cargo-dist) from `dist-workspace.toml`. Pushing a tag such as `v0.1.0` runs `.github/workflows/v-release.yml`, which dist generates. That workflow is the only thing in the repository that creates a GitHub Release or publishes a package.

## What a release contains

The release workflow builds `bugsplat` for six targets and attaches these files to a GitHub Release:

| File | Contents |
| --- | --- |
| `bugsplat-cli-aarch64-apple-darwin.tar.gz` | macOS on Apple silicon |
| `bugsplat-cli-x86_64-apple-darwin.tar.gz` | macOS on Intel |
| `bugsplat-cli-x86_64-unknown-linux-musl.tar.gz` | Linux x64, statically linked |
| `bugsplat-cli-aarch64-unknown-linux-musl.tar.gz` | Linux arm64, statically linked |
| `bugsplat-cli-x86_64-pc-windows-msvc.zip` | Windows x64 |
| `bugsplat-cli-aarch64-pc-windows-msvc.zip` | Windows arm64 |
| `bugsplat-cli-npm-package.tar.gz` | The npm package `@bugsplat/cli` |
| `bugsplat-cli-installer.sh`, `bugsplat-cli-installer.ps1` | Install scripts for a shell and for PowerShell |
| `*.sha256`, `sha256.sum`, `source.tar.gz` | Checksums and the source |

The Linux binaries are statically linked, so the same file runs on glibc distributions and on Alpine. `bugsplat` verifies HTTPS connections with the CA certificates installed on the system. An image without them, such as `node:22-slim`, needs the `ca-certificates` package, or every command that talks to BugSplat fails with `No CA certificates were loaded from the system`.

The workflow does not publish to npm. A person publishes the npm package from the GitHub Release (see "Publishing to npm").

We have not run either install script. Run both against the first prerelease before you document them.

## How the npm package works

npm is the primary way to install the CLI: `npm install -g @bugsplat/cli`.

- The package contains no binary. Its `postinstall` script downloads the archive for your platform from the GitHub Release with the same version and unpacks it inside the package folder. A package manager that skips install scripts gets the same download the first time `bugsplat` runs.
- The download comes from `https://github.com/BugSplat-Git/bugsplat-cli/releases/download/v<version>/`. While the repository is private that URL returns 404 and `npm install` fails with `Error fetching release: HTTP 404`. The npm package cannot work until the repository is public.
- The `bugsplat` command that npm adds is a Node.js script that starts the downloaded binary and passes its arguments, output, and exit code through. You need Node.js 14.18 or later to install the package and every time you run `bugsplat`. On an Apple silicon Mac, `bugsplat --version` took about 0.15 seconds through the script and about 0.02 seconds when we ran the binary directly.
- Unpacking uses the system `tar` on macOS and Linux and PowerShell's `Expand-Archive` on Windows. We ship `.tar.gz` archives because `tar` in slim container images, such as `node:22-slim`, cannot unpack `.tar.xz`.
- The package has one dependency, `detect-libc`.
- The package's README is this repository's `README.md`, so the "first release is not out yet" notice has to come out of the README before the first publish.

A machine without Node.js can use the archive for its platform from the GitHub Release. The archive holds the `bugsplat` binary, which runs without Node.js.

## Build every target without releasing

The "Build release artifacts" workflow (`.github/workflows/build-artifacts.yml`) builds everything in the table above and keeps the files as workflow artifacts for seven days. It never creates a tag, a GitHub Release, or a package. The workflow also:

- runs `.github/scripts/smoke-test.sh` on each binary on a runner of its own platform: `bugsplat --version`, `bugsplat symbols info` on committed PDB, PE, ELF and dSYM fixtures, and `whoami` and `logout` against a session in the file token store. On the runner, the script also checks the operating system's credential store: that the macOS keychain and the Windows Credential Manager answer, and that `whoami` and `logout` use the session file on a Linux machine with no Secret Service. Storing a session needs `bugsplat login`, so the script does not cover it;
- installs the generated npm package into a temporary project with `.github/scripts/npm-package-test.sh` and runs `bugsplat --version` through it. The script points the package's download URL at the archives the run just built, because the GitHub Release does not exist yet.

The workflow runs only when you start it:

```bash
gh workflow run build-artifacts.yml --ref main
gh run watch
```

The workflow does not run for pull requests. GitHub bills macOS minutes at 10 times and Windows minutes at 2 times the Linux rate in a private repository, and one run uses two macOS runners and two Windows runners.

You can run both scripts on your own machine after `dist build`:

```bash
dist build --artifacts=local --target=aarch64-apple-darwin
dist build --artifacts=global
.github/scripts/smoke-test.sh target/distrib/bugsplat-cli-aarch64-apple-darwin.tar.gz aarch64-apple-darwin
.github/scripts/npm-package-test.sh target/distrib
```

Both scripts set `BUGSPLAT_TOKEN_STORE=file` and a temporary `BUGSPLAT_CONFIG_DIR`, so they never read your keychain or your real session.

## Before the first release

Each of these needs a person with access we do not give to automation.

1. **Make the repository public.** The npm package and both install scripts download from GitHub Releases, which only works for a public repository.
2. **Choose who publishes to npm.** That person needs permission to publish packages in the `@bugsplat` organization on npmjs.com. See "Publishing to npm".
3. **Agree on the version.** `Cargo.toml` says `0.1.0`.
4. **Sign the macOS binaries.** See "macOS signing and notarization". The certificate has to come from our Apple Developer account.
5. **Update the README.** Remove the "first release is not out yet" notice. The npm command and the build-from-source section both stay.

## Publishing to npm

A person publishes each version, from the package file the release built:

```bash
gh release download v0.1.0 --pattern bugsplat-cli-npm-package.tar.gz
npm publish --access public bugsplat-cli-npm-package.tar.gz
```

Run `npm login` first. `--access public` is required because npm publishes a scoped package as private by default.

We publish by hand for two reasons:

- npm's [trusted publishing](https://docs.npmjs.com/trusted-publishers), which lets a workflow publish without a stored token, is set up on the package's settings page. The package has to exist first, so someone has to publish the first version.
- dist 0.34.0 can generate an npm publish job (`publish-jobs = ["npm"]`), and that job signs in with an `NPM_TOKEN` secret on Node.js 20. npm's [access token page](https://docs.npmjs.com/about-access-tokens) says publishing new versions directly with a token will be removed in January 2027.

To publish later releases from the workflow, the repository needs its own publish job in place of dist's: Node.js 22.14 or later, npm 11.5.1 or later, the `id-token: write` permission, and a trusted publisher on npmjs.com that names this repository and its release workflow. We have not written or tested that job.

## macOS signing and notarization

### Why the binaries need a signature

`bugsplat login` stores your session in the macOS keychain, and macOS ties a keychain item to the code signature of the program that wrote it. dist's Apple silicon build carries only the ad hoc signature the linker adds. Its requirement is the hash of that exact file:

```
$ codesign -d -r- bugsplat
# designated => cdhash H"439d97dec17495354567f11528f6e0afac5bef42"
```

Every new version has a new hash, so after every upgrade macOS asks for the login password before `bugsplat` can read the session it stored. A binary signed with a Developer ID certificate has a requirement that names the team and the identifier `bugsplat`, which stays the same from one version to the next.

### What signing needs

1. Our membership in the Apple Developer Program.
2. A **Developer ID Application** certificate, exported from Keychain Access with its private key as a `.p12` file with a password.
3. Three repository secrets:

   | Secret | Value |
   | --- | --- |
   | `CODESIGN_CERTIFICATE` | The `.p12` file, base64 encoded: `base64 -i certificate.p12 \| pbcopy` |
   | `CODESIGN_CERTIFICATE_PASSWORD` | The password of the `.p12` file |
   | `CODESIGN_IDENTITY` | The certificate's name, in the form `Developer ID Application: <team name> (<team ID>)` |

4. In `dist-workspace.toml`, uncomment `macos-sign = true` and `github-build-setup`, run `dist generate`, and commit both files.

Add the secrets before you turn on `macos-sign`. dist reads an unset GitHub secret as an empty value and tries to sign with it. We read this in dist 0.34.0's source and have not run it.

`github-build-setup` adds the step in `.github/release-build-setup.yml` to the release build. The step sets `CODESIGN_OPTIONS=runtime`, which makes dist sign with the hardened runtime. Apple's notary service rejects a binary without it.

With signing on, dist runs `codesign` on the two macOS binaries during the release build. The "Build release artifacts" workflow never receives the secrets, so its macOS binaries stay unsigned. To see a signed build before the real release, push a prerelease tag (step 3 of the checklist).

Check a signed binary with:

```bash
codesign --verify --strict --verbose=2 bugsplat
codesign -d -r- bugsplat
```

The second command has to print a requirement that contains `identifier bugsplat` and your team ID, with no `cdhash`.

We have not signed a build, because we have no certificate. Everything in this section comes from dist 0.34.0's source and from a dry run of `dist generate`.

### Notarization

dist 0.34.0 signs and does not notarize. Notarization is a separate decision:

- npm and `curl` do not mark the files they download as quarantined, so macOS runs a signed `bugsplat` installed either way without checking for notarization.
- A browser does mark its downloads. Someone who downloads the `.tar.gz` from the Releases page in a browser gets a warning that macOS cannot verify the program unless the binary is notarized.

To notarize a release by hand, you need an App Store Connect API key (the `.p8` file, its key ID, and the issuer ID) for the same team. Then, for each of the two macOS archives of the release:

```bash
tar -xzf bugsplat-cli-aarch64-apple-darwin.tar.gz
ditto -c -k bugsplat-cli-aarch64-apple-darwin/bugsplat bugsplat.zip
xcrun notarytool submit bugsplat.zip --key AuthKey.p8 --key-id <key id> --issuer <issuer id> --wait
```

Notarization records the signed binary with Apple and does not change the file, so you can do it after the release is out. A bare binary cannot carry a stapled ticket, so macOS looks the notarization up online the first time the binary runs. We have not run these commands.

## Release checklist

1. **Set the version.** Change `version` under `[workspace.package]` in `Cargo.toml`, run `cargo check` so `Cargo.lock` follows, and merge the change to `main` through a pull request. The tag has to match this version.
2. **Build every target.** Run the "Build release artifacts" workflow on `main` and wait for every job to pass.
3. **Rehearse with a prerelease** when the release workflow or signing changed since the last release. Set the version to a prerelease such as `0.2.0-rc.1`, tag it `v0.2.0-rc.1`, and push the tag. The workflow creates a GitHub prerelease. A prerelease works while the repository is private, but only people with access to the repository can download its files. Check the files, then delete the prerelease and its tag if you do not want to keep them.
4. **Tag the release** on the `main` commit from step 1:

   ```bash
   git switch main && git pull
   git tag v0.1.0
   git push origin v0.1.0
   ```

5. **Watch the Release workflow** with `gh run watch`. It builds the six targets and creates the GitHub Release.
6. **Check the GitHub Release.** It has the files listed under "What a release contains".
7. **Publish the npm package** with the two commands under "Publishing to npm", then check it in an empty folder:

   ```bash
   npm view @bugsplat/cli version
   npm install @bugsplat/cli
   npx bugsplat --version
   ```

   `bugsplat --version` prints the version and the target, for example `bugsplat 0.1.0 (aarch64-apple-darwin)`. Repeat on Windows and on Linux.
8. **Check the macOS signature** with the two `codesign` commands above, on the binary npm installed: `node_modules/@bugsplat/cli/node_modules/.bin_real/bugsplat`.
9. **Sign in with the released binary** on macOS: `bugsplat login`, then `bugsplat whoami`. After the next release, upgrade and run `bugsplat whoami` again. macOS should not ask for your password.

npm never lets a version number be reused, so a release that published a broken package needs a new patch version.

## Toolchain versions

- Rust is pinned in `rust-toolchain.toml`. CI, the release builds, and your machine all read that file.
- dist is pinned by `cargo-dist-version` in `dist-workspace.toml`. Install the same version locally before you run `dist generate`, because the release workflow fails when `v-release.yml` differs from what its dist version generates.
- The actions in `v-release.yml` are pinned to commits under `[dist.github-action-commits]` in `dist-workspace.toml`. Dependabot does not update that workflow; change the pins there and run `dist generate`.
