# RedForge

A map editor for **RedM** (Red Dead Redemption 2), by Prysma Studio. This is the
Rust rewrite of the C#/WinUI RedForge, which grew out of CodeX. It runs on Linux
and Windows.

It is early work. Today:

- every file format RedForge handles can be read, written and converted to and
  from XML, matching the C# RedForge;
- the app opens the game, browses its files and shows models, textures and XML;
- the world map streams the game's map around a free camera (View → World map),
  with its LODs, interiors, terrain and time of day; a ymap opens on its own;
- entities can be selected, moved, rotated, scaled, edited in Properties,
  duplicated, copied, deleted and placed, with undo; edited ymaps and ytyps
  are saved with "Save as…" in Properties;
- the Objects pane lists the game's archetypes with 3D thumbnails, to search
  and choose the one to place.

Projects (`.rfproj`), RedM export and the other panes come in later milestones.

## Running it

```sh
cargo run --release -p rf-app
```

It asks for the game folder the first time (or takes `REDFORGE_GAME_DIR`), and
keeps it in `RedForge.Settings.ini` in the user's config folder.

In the map: drag with the mouse to look around, WASD to move, R/F for up/down,
Shift to go faster, the wheel to change speed. "Go to…" jumps to a town.

Editing (the keys are the C# RedForge's):

| | |
|---|---|
| Right click (or click) | Select what's under the mouse; with Ctrl, add or remove it |
| Q, E, toolbar | Move, rotate (Scale and Place are in the toolbar); V back to selecting |
| `.` | Gizmo along the world's or the object's axes |
| Shift at the start of a gizmo drag | Drag copies |
| Delete, Ctrl+D, Ctrl+C, Ctrl+V | Delete, duplicate, copy, paste (into the ymap chosen in Properties) |
| Ctrl+Z, Ctrl+Y | Undo, redo |
| Home | Look at the selection |
| Esc | Clear the selection; leave the Place tool |

To place something, find it in Objects (beside Properties, or View → Objects):
the search matches the names and the ytyps they're in, and the list can be
narrowed to props, drawable dictionaries, fragments or interiors. Clicking an
object picks the Place tool with it; then a click in the map places it in the
ymap chosen under "Place in" (Q/E turn it, a right click stops).

Editing an entity of a game ymap makes that ymap edited: the map shows the
edited data from then on. Deleting fixes the LOD indices (`parentIndex`,
`numChildren`) in the ymap, its parent and its children, and refuses entities
that still have LOD children. Interior entities are edited in their interior's
ytyp, so every placement of that interior changes.

For checking the app without a screen, it can open something, take a screenshot
of its window and quit:

```sh
REDFORGE_OPEN="world:Valentine" REDFORGE_SCREENSHOT=shot.png REDFORGE_SCREENSHOT_DELAY=25 \
    target/release/redforge
```

`REDFORGE_OPEN` is a game path (a model, texture or ymap), `world`,
`world:<place>` or `world:x,y,z[,yaw,pitch]`. `REDFORGE_SELECT_CENTRE=1`
selects what's in the middle of the world map before the screenshot, and
`REDFORGE_TEST_DRAG=1` then drags the gizmo with made-up mouse events.
`REDFORGE_OBJECTS_SEARCH=<text>` shows the Objects pane beside the world map
with that search, and chooses the first object.

## Crates

| Crate | What it is |
|---|---|
| `rf-formats` | The game's file formats as plain data: RPF8 archives; RSC8 resources (ydr, ydd, yft, ytd, ybn, ycd, yad, yld, yed, ynv, yfd, ypmd, yldb, yvr, ywr); metadata (ymap, ytyp, ymt, PSO, RBF); fxdb; DDS. No rendering, no UI. |
| `rf-game` | The game's data layer: DLC packs, data files, stream entries, archetypes, texture dictionary relationships (GTXD, HD bindings). |
| `rf-world` | The world: map nodes, ymaps and their entities, the LOD tree, interiors, cull boxes and map states, streamed around a camera. |
| `rf-editor` | The editor: the ymaps and ytyps being edited, the selection, undo and redo, LOD-safe deletes, saving. No UI. |
| `rf-render` | The renderer, on wgpu (Vulkan on Linux, Vulkan/DX12 on Windows) with WGSL shaders: textures, models, RDR2's materials (standard, two-layer, decals, tint palettes, terrain). |
| `rf-app` | The `redforge` app: egui, with Game Files, the model, texture and XML viewers, the map, Properties and Objects. |
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
