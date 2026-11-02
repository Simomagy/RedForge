# RedForge

A map editor for **RedM** (Red Dead Redemption 2), by Prysma Studio. This is the
Rust rewrite of the C#/WinUI RedForge, which grew out of CodeX. It runs on Linux
and Windows.

It is early work. Today:

- every file format RedForge handles can be read, written and converted to and
  from XML, matching the C# RedForge;
- the app opens the game, browses its files and shows models, textures and XML.

The map and the editor come in later milestones.

## Running it

```sh
cargo run --release -p rf-app
```

It asks for the game folder the first time (or takes `REDFORGE_GAME_DIR`), and
keeps it in `RedForge.Settings.ini` in the user's config folder.

## Crates

| Crate | What it is |
|---|---|
| `rf-formats` | The game's file formats as plain data: RPF8 archives; RSC8 resources (ydr, ydd, yft, ytd, ybn, ycd, yad, yld, yed, ynv, yfd, ypmd, yldb, yvr, ywr); metadata (ymap, ytyp, ymt, PSO, RBF); fxdb; DDS. No rendering, no UI. |
| `rf-game` | The game's data layer: DLC packs, data files, stream entries, archetypes, texture dictionary relationships (GTXD, HD bindings). |
| `rf-render` | The renderer, on wgpu (Vulkan on Linux, Vulkan/DX12 on Windows) with WGSL shaders: textures, models, RDR2's materials. |
| `rf-app` | The `redforge` app: egui, with Game Files, model, texture and XML viewers. |
| `rf-cli` | The `rf` command: list, extract and check game files. |

## The `rf` command

```sh
cargo build --release
export REDFORGE_GAME_DIR="/path/to/Red Dead Redemption 2"

target/release/rf ls ymap                  # every path containing "ymap"
target/release/rf ls -l van_farmhouse      # with sizes, compression, encryption, version
target/release/rf ls --archives update_4   # archives instead of files
target/release/rf extract --filter twi_01_strm -o out           # decompressed data
target/release/rf extract --loose --filter twi_01_strm -o out   # as RedM loads it from stream/
target/release/rf check --limit 20000      # decrypt + decompress a sample, report failures
target/release/rf stats
target/release/rf hash prop_name
target/release/rf xml out/foo.ydr                  # loose file -> foo.ydr.xml (+ foo/ with the DDS textures)
target/release/rf from-xml out/foo.ydr.xml         # and back
```

`rf compare <ext>` checks the port against the C# RedForge on game files (see
[docs/PORTING.md](docs/PORTING.md)). [docs/CSHARP_BUGS.md](docs/CSHARP_BUGS.md)
lists the C# bugs the port keeps for compatibility.

Paths are `/`-separated and start from the game folder, like
`levels_3.rpf/levels/rdr3/interiors/van_farmhouse01_int.rpf/van_03_monster.ydr`.
Case and `\` don't matter when looking a path up.

## Data files

- `data/rdr2/strings.txt`: the names the hashes in the game files stand for.
  RPF8 stores no names, only hashes, so a path is only readable if its name is in
  here; unknown ones show up as `0x<HASH>.<ext>`.
- `crates/rf-formats/data/magic.dat`: the RPF8 cipher keys and tables, obfuscated
  and encrypted. They are only usable with the AES key found in the user's own
  `RDR2.exe`, which is why the game folder is needed.

## Tests

```sh
cargo test                                          # unit tests
REDFORGE_GAME_DIR="/path/to/Red Dead Redemption 2" cargo test   # plus tests on the real game files
```

## License

MIT OR Apache-2.0, at your option.
