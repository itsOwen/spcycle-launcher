# SPCycle Launcher

A singleplayer launcher for The Cycle: Frontier. It downloads the game, sets up a
local server and starts everything for you.

Windows and Linux. No administrator rights needed.

## Requirements

- [Node.js](https://nodejs.org) 24+ and [pnpm](https://pnpm.io) 11+
- [Rust](https://rustup.rs) (stable)
- Linux only: `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`,
  `patchelf`
- About 40 GiB of free disk space for the game

## Build

```sh
pnpm install
./tools/fetch-depot-blob.sh
pnpm tauri dev            # run it
pnpm tauri build          # or build an installer
```

`fetch-depot-blob.sh` grabs the depot manifest the launcher needs. It is not in
this repo, so run it once before building. Set `DEPOT_BLOB_URL` to fetch it from
your own host.

## Release

```sh
./tools/release.sh v0.1.0            # build into out/
./tools/release.sh v0.1.0 --publish  # and publish it
```

Needs a signing key in `.secrets/`:

```sh
pnpm tauri signer generate -w .secrets/spcycle.key
```

Keep a backup of it somewhere safe. Without it you cannot ship updates that
existing installs will accept.

## Tools

| | |
|---|---|
| `tools/fetch-depot-blob.sh` | fetch the depot manifest |
| `tools/repack-mongod.sh` | build the bundled mongod archives |
| `tools/publish-components.sh` | publish the component assets to their own fixed tag |
| `tools/release.sh` | build and publish a release |
| `tools/check-windows.sh` | type-check the Windows-only code from Linux |
| `tools/build-appimage.sh` | build the AppImage in docker |

## Credits

The local server and the client loader come from
[deiteris/Prospect](https://github.com/deiteris/Prospect).
