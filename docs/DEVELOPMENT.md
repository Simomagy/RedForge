# Developing RedForge

## Building and running

```sh
cargo run --release -p rf-app      # the app
cargo build --release              # the app and the rf tool, in target/release/
```

The debug profile optimises the dependencies (the RPF cipher and the Kraken
decompressor are unusably slow without it), so `cargo run -p rf-app` is
usable too.

## Crates

| Crate | What it is |
|---|---|
| `rf-formats` | The game's file formats as plain data: RPF8 archives; RSC8 resources (ydr, ydd, yft, ytd, ybn, ycd, yad, yld, yed, ynv, yfd, ypmd, yldb, yvr, ywr); metadata (ymap, ytyp, ymt, PSO, RBF); fxdb; DDS. No rendering, no UI. |
| `rf-game` | The game's data layer: DLC packs, data files, stream entries, archetypes and their lights, materials, texture dictionary relationships (GTXD, HD bindings), peds put together from their metapeds, the timecycle; a project's files on top. |
| `rf-world` | The world: map nodes, ymaps and their entities, the LOD tree, interiors, instanced batches, cull boxes and map states, streamed around a camera; what the selection modes show of the ymaps. |
| `rf-editor` | The editor: projects, the ymaps and ytyps being edited, the selection, undo and redo, LOD-safe deletes, saving, the RedM export. No UI. |
| `rf-bridge` | The Blender add-on's link: its protocol (the C#'s) over TCP, the models to and from Sollumz XML. |
| `rf-render` | The renderer, on wgpu (Vulkan on Linux, Vulkan/DX12 on Windows) with WGSL shaders: textures, models, RDR2's materials; skinned models posed by clips; deferred lighting with cascaded shadows, SSAO, lights, the sky and the HDR post-processing; outlines and the editor's helpers. |
| `rf-app` | The `redforge` app: egui, with Game Files, the viewers, the map, Project, Hierarchy, Properties, Objects, the loading screen and the tour. |
| `rf-cli` | The `rf` command: list, extract, check and convert game files. |

## Porting from the C# RedForge

The C# RedForge is the reference: formats are ported to match it byte for
byte. `rf compare <ext>` checks the port against it on game files (see
[PORTING.md](PORTING.md)); [CSHARP_BUGS.md](CSHARP_BUGS.md) lists the C#'s bugs
the port keeps for compatibility.

## Data files

- `data/rdr2/strings.txt`: the names the hashes in the game files stand for.
  RPF8 stores no names, only hashes, so a path is only readable if its name is in
  here; unknown ones show up as `0x<HASH>.<ext>`. The app looks for it next to
  its executable (`data/`), in `../share/redforge/data` from it (the Linux
  packages), or in the source tree; `REDFORGE_DATA` overrides.
- `data/rdr2/schemas.txt`, `data/rdr2/mapstates.txt`: built into the binaries.
- `crates/rf-formats/data/magic.dat`: the RPF8 cipher keys and tables, obfuscated
  and encrypted. They are only usable with the AES key found in the user's own
  `RDR2.exe`, which is why the game folder is needed.

## Tests

```sh
cargo test                                                      # unit tests
REDFORGE_GAME_DIR="/path/to/Red Dead Redemption 2" cargo test   # plus tests on the real game files
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

## Screenshots without a screen

The app can open something, take a screenshot of its window and quit:

```sh
REDFORGE_OPEN="world:Valentine" REDFORGE_SCREENSHOT=shot.png REDFORGE_SCREENSHOT_DELAY=25 \
    target/release/redforge
```

| Variable | What it does |
|---|---|
| `REDFORGE_GAME_DIR` | The game folder (instead of the saved one) |
| `REDFORGE_OPEN` | `welcome` (nothing), `folder` (the game folder's dialog), a game path (a model, texture, clip dictionary or ymap; `hex:<path>` for the hex viewer), `world`, `world:<place>` or `world:x,y,z[,yaw,pitch]` |
| `REDFORGE_SCREENSHOT`, `REDFORGE_SCREENSHOT_DELAY` | Where the screenshot goes, and how many seconds after opening (6 by default) |
| `REDFORGE_HOUR` | The map's hour |
| `REDFORGE_MODE` | A selection mode to start in (`Physics`, `Navmesh`...) |
| `REDFORGE_SELECT_CENTRE=1` | Selects what's in the middle of the map (the helper, in a selection mode) |
| `REDFORGE_TEST_DRAG=1` | Then drags the gizmo with made-up mouse events and logs what happened (`=copy`: with Shift, dragging copies) |
| `REDFORGE_TEST_QUIT=1` | Closes the window 1.5 s before the screenshot (to see the question about unsaved changes) |
| `REDFORGE_TEST_KEY` | Presses a key a second before the screenshot: `F1`, `ctrl+d`... |
| `REDFORGE_OBJECTS_SEARCH` | Shows Objects with that search and chooses the first object (the Place tool) |
| `REDFORGE_SHOW_TAB` | Brings a pane to the front (Project, Hierarchy...) |
| `REDFORGE_PROJECT` | A project to open instead of the last one (not remembered) |
| `REDFORGE_HOLD_LOADING` | Keeps the loading screen up that many seconds |
| `REDFORGE_DROP_HOVER=1` | Shows the card files dragged over the window get |
| `REDFORGE_TOUR` | Shows the tour at that step (0-7), when it hasn't been seen |
| `REDFORGE_ANIM_PED`, `REDFORGE_ANIM_TIME` | For a clip dictionary: the ped, and the time to stop the clip at |

## Packages

Everything builds from Linux, into `target/` (`target/dist/` for the
releases):

| Package | Command | Needs |
|---|---|---|
| Arch (this checkout) | `makepkg -si` in `packaging/arch` | `base-devel` |
| AUR `redforge-git`, `redforge-bin` | `packaging/aur/*/PKGBUILD` | an AUR account |
| Tarball, `.deb`, AppImage | `packaging/linux/build.sh [tarball\|deb\|appimage\|all]` | `zig`, `cargo-zigbuild`, `cargo-deb`, `appimagetool` |
| Flatpak bundle | `packaging/flatpak/build.sh` | `flatpak-builder` (the SDK and Rust extension come from Flathub) |
| Windows MSI | `packaging/windows/build-msi.sh` | `cargo-xwin`, `wixl` (msitools) |

The Linux binaries of `build.sh` are built for glibc 2.31 (Ubuntu 20.04,
Debian 11 and newer); a plain `cargo build` on Arch needs the newest glibc.

The packages point to the repository, `github.com/Simomagy/RedForge`: the AUR
packages' sources, the Flatpak's id (`io.github.Simomagy.RedForge`) and its
metainfo.

The projects' icon is drawn in `packaging/linux/icons/redforge-project.svg`;
`packaging/linux/icons/project-icon.sh` renders its PNGs and the Windows
`crates/rf-app/assets/redforge-project.ico` (`rsvg-convert`, `magick`).

## Versions and releases

The versions follow [Semantic Versioning](https://semver.org) from 2.0.0: the
major number for changes that break projects or files, the minor for new
features, the patch for fixes. `CHANGELOG.md` is in the
[Keep a Changelog](https://keepachangelog.com) form: what changes goes under
`[Unreleased]` as it's made, and at a release that section becomes the
version's, which pastes as is into the GitHub release.

The version lives in:

- `Cargo.toml` (`[workspace.package] version`): the app, `rf`, the MSI, the
  `.deb`, the AppImage and the tarball take it from there, and so does
  `packaging/arch/PKGBUILD` (its `pkgver()`);
- `packaging/aur/redforge-bin/PKGBUILD` (`pkgver`, then its `.SRCINFO`);
- `packaging/flatpak/redforge.metainfo.xml` (a `<release>` per version).

A release, then:

1. Move `[Unreleased]` to `[x.y.z] - <date>` in `CHANGELOG.md` (with its link
   at the bottom), bump the version in the places above, commit.
2. Tag it: `git tag vx.y.z && git push origin master vx.y.z`.
3. Build: `packaging/linux/build.sh all`, `packaging/flatpak/build.sh`,
   `packaging/windows/build-msi.sh`.
4. Make the GitHub release from the tag, its notes the version's changelog
   section, and attach `target/dist/*` and `target/msi/*.msi`.
5. The AUR: put the tarball's checksum in
   `packaging/aur/redforge-bin/PKGBUILD` (`updpkgsums`), regenerate both
   `.SRCINFO` files (`makepkg --printsrcinfo > .SRCINFO`) and push each package
   to its own AUR repository (`git clone ssh://aur@aur.archlinux.org/<name>.git`,
   copy the two files in, commit, push).
