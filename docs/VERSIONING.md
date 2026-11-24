# Versioning, releases and updates

Same flow as debutuuru. One version for the whole product, in
`[workspace.package]` in the root `Cargo.toml`; never bump it by hand.

## Commit messages (Conventional Commits)

| Prefix | Effect (while < 1.0) |
| --- | --- |
| `fix:` | patch (0.2.0 -> 0.2.1) |
| `feat:` | minor (0.2.1 -> 0.3.0) |
| `docs:`, `chore:`, `refactor:`, `test:`, `ci:` | no release |

## Branches

| Branch | Workflows | Use |
| --- | --- | --- |
| `develop` | none | Day-to-day commits. |
| PR `develop` -> `main` | CI | The gate before anything reaches `main`. |
| `main` | CI + Release PR | Only merges from `develop` and the release PR. |
| tag `vX.Y.Z` | Release (installer) | Created by merging the release PR. |

## Flow

1. Commit and push on `develop`. When ready: **Actions > Ship > Run workflow**.
   It opens the PR `develop` -> `main` with auto-merge (merge commit).
2. release-plz opens a **Release PR** (version + `CHANGELOG.md`) with auto-merge.
3. Merging it tags `vX.Y.Z`; `release.yml` builds the installer, signs it,
   writes `latest.json` and publishes the GitHub Release.
4. Installed copies see the new version (`gamennetto update`, and a notice
   at most once a day when `host`/`view` start). The installer is checked
   against the public key built into the program before it runs.
5. The release is merged back into `develop`: `git pull` before committing again.

## Installing on another PC

Download `Gamennetto-X.Y.Z-setup.exe` from
https://github.com/wilder00/gamennetto/releases/latest and run it. No
administrator rights needed; it adds Start menu shortcuts and puts
`gamennetto` on PATH. Later: `gamennetto update`.

## One-time setup (GitHub)

- **Token**: fine-grained token for this repo (contents + pull requests:
  write), stored as secret `RELEASE_PLZ_TOKEN`.
- **Auto-merge**: Settings > General > "Allow auto-merge" (and merge commits).
  Ruleset on `main`: require a pull request (merge method: merge) and the
  status check `test`.
- **Update signing key** (run by the maintainer, never by CI):
  `cargo run -p xtask -- keygen` (asks for a password; writes
  `~/.gamennetto/update.key` and `update.key.pub`, and prints what to store)
  - secret `GAMENNETTO_SIGNING_KEY`: the private key file's content
  - secret `GAMENNETTO_SIGNING_KEY_PASSWORD`: its password
  - variable `UPDATE_PUBKEY`: the public key (`update.key.pub` content)

  Keep a backup of the private key: without it, installed copies can't be
  updated any more.
