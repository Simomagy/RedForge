# Changelog

All notable changes to RedForge are written here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): the major number
for changes that break projects or files, the minor for new features, the
patch for fixes. Each version's section pastes as is into its GitHub release.

## [Unreleased]

### Added
- Exporting a project to a RedM resource bakes an **ambient mask volume** for
  each of its interiors (those its ytyps have and its ymaps place), into an
  `amv_zone_<n>.ymt` and `.ytd` in the resource's stream folder: nothing to
  do by hand, and they follow the interior when it changes. The zone's number
  is the project's **Ambient zone** (Project pane, RedM resource): 183 unless
  changed. The game's own zones are 0 to 182 and it loads the ones after them
  only with none missing, so a second resource on the same server takes 184.
  A project with an `amv_zone` file of its own is left alone. The export also says what else an
  interior's lighting lacks next to the game's own (below).
- `rf amv-bake` bakes an **ambient mask volume** for an interior that has
  none. Without one the game lights an interior as if it stood in the open,
  and its metals (lamps, pitchers, window frames) come out black by day. From
  the interior's ytyp, the ymap that places it and the ydr models next to
  them, it casts rays from every cell of a box around the interior and writes
  how much of the sky each cell sees in six directions, as the game's own
  volumes have it: an `amv_zone_<n>.ymt` and `.ytd` to put in the resource's
  stream folder, numbered after the game's own zones (183 unless `--zone`
  says otherwise) so that none of those is replaced. Glass and decals let the light through; the models drawn
  only into shadows or reflections are left out.
  It also says what else the interior's lighting lacks next to the game's
  own interiors: rooms with no time cycle, rooms with no reflection probe,
  and entities with neither of the flags the game's interior entities have.

## [2.4.0] - 2026-11-20

### Added
- The animation editor keys the way Blender does. Every number of a bone
  (its location's X, Y, Z, its rotation's W, X, Y, Z or Euler angles, counted
  from the rest pose) has an F-curve of keyframes with Bézier handles:
  Constant, Linear or Bézier to the next key, and Free, Aligned, Vector,
  Automatic or Auto Clamped handles, worked out as Blender works them out.
  Below the view, a **Dope Sheet** (a row a bone, opened into its channels,
  under a Summary) and a **Graph Editor** (the curves, with the selected
  keys' handles to drag): click and Shift+click select, a drag from a key
  moves the selected keys and one from nothing selects in a box, G grabs,
  Shift+D duplicates, X deletes, A and Alt+A select all and none, T sets the
  interpolation and V the handles, the ruler scrubs, the wheel zooms, the
  middle button pans and Home shows everything. On the right, the bone's
  **Item** panel: Location and Rotation fields, green when animated, yellow
  on a key and orange when changed and not keyed, with the keyframe menu on
  a right click, the rotation mode, and the **Active Keyframe**'s
  interpolation, frame, value and handles. **Auto Key** (the record button)
  keys every change; off, I keys the bone and a change not keyed goes when
  the frame changes. Alt+I deletes the bone's keys at the frame, Alt+G and
  Alt+R put it back at rest, the arrows step frames and jump between keys.
  In the view, **G** moves the selected bone (with Reach, its limb to where
  it's pulled) and **R** turns it, following the mouse until a click or Enter
  keeps it and a right click or Esc puts it back; X, Y and Z keep to an axis
  of the model, pressed again to the bone's own. There's no scale: the game
  takes none from a clip.
  Animations saved before open as they were, their keys turned into curves.
- Animations in a project: **Add to project** in the animation editor (or
  Add files…, or dropping the `.rfanim.json` on the window) lists the
  animation under Animations in the Project pane, where a double click opens
  it. **Export RedM resource** then bakes each one into the resource's `stream`
  folder as the ycd the game plays, named after its dictionary; an animation
  that can't be baked stops the export, saying why, and one open with
  changes not saved is baked as last saved, the export saying so. An
  animation saved into a folder of the project joins it by itself.
- **File > New animation**: the animation editor, the first of an animation
  creator. A ped stands with its joints showing; click one (or pick the bone
  from the list), turn it with the gizmo, and that's a key at the frame
  shown. Go to another frame, turn it again: RedForge works out what is
  between the keys (smoothly, evenly or held, for each key). The timeline
  shows the keys, Space plays, K keys the bone as it is, and Undo takes back
  a turn. With **Reach** (R) the gizmo moves the bone instead and the two
  above it follow: pull a hand and its arm bends to get there, a foot and
  its leg, the hand or foot staying turned as it was unless **Keep its turn**
  is off (T turns again). **Save** keeps the keys (`.rfanim.json`, opened
  again with **File > Open animation…**) and **Export ycd** writes the
  dictionary the game plays, for a resource's `stream` folder. Only the bones
  with keys are animated: in game the rest of the ped stays as it was.
  **Base pose** stands the ped in a moment of one of the game's clips
  (search the clip dictionaries by name, choose the clip and the time), to
  see the animation as it will look over what the ped is doing; **Key this
  pose** makes it part of the animation, a key for every bone the clip
  moves, for an animation of the whole body.
- `rf anim-bake` makes the ycd the game plays from an animation written as
  keys: for each bone, rotations and positions at frames (JSON), with the
  values between them worked out (smooth, linear or held). Only the bones
  with keys are animated, the rest of the ped staying as it is: a gesture
  takes a few lines. The first piece of the animation creator; the files it
  makes play in RedM.
- `rf skeleton` lists a ped's bones from the game's files (`rf skeleton
  player`, `male`, `female`, or a yft): each one's tag, parent and rest pose,
  in the units and axes an animation's keys are written in; `--json` for
  tools.

### Fixed
- The animation viewer showed a ped playing one of the game's clips from
  behind: the clip's root is given from the ped's own place in it, which is
  turned half way round, and that wasn't applied.


## [2.3.6] - 2026-11-16

### Added
- The version installed is shown at the right end of the status bar.
- Objects can be placed in an interior placed in one of the edited ymaps:
  the new object goes in the interior's archetype, in the room it's placed
  in.
- Trees and bushes (what the game places in prop batches) are added as
  instances of their ymap's batch, made if there's none.
- A new interior placed on the map gets what the game's interior ymaps
  have: their flags and its archetype's exit portals. A ytyp from Sollumz
  RDR added to a project gets what the game's interiors have and Sollumz
  has no fields for.
- RedForge for Blender makes breakable props, doors and gates: **As**, next
  to Create Archetype, gives the archetype what the game's own have (a
  breakable prop, a swinging door, a sliding one), and a game fragment
  comes back from Blender as the game made it.

### Changed
- The Blender add-on is now **RedForge for Blender**, one add-on in place of
  RedForge Bridge and Sollumz RDR (disable those): install
  `blender/redforge_blender.zip` with Preferences > Get Extensions > Install
  from Disk, in Blender 5.3 or newer. A fork of Sollumz RDR for Red Dead
  Redemption 2 only, with one RedForge tab: the link with RedForge, files and
  view on top, the tools by kind of file under them.

### Fixed
- The game crashed loading a ymap that was saved empty and given objects
  later (a new ymap, saved, then filled): it was written without the enums of
  its entities. Saving such a ymap again puts them back.
- Blender 5.3 crashed drawing a building brought in with Edit in Blender (a
  tint attribute per material: the materials with the same palette share one
  now).
- A model's collision material came back from Blender as DEFAULT (and showed
  pink there): RedForge writes it as its hash, which the add-on now reads.
- Fragments came back from Blender with GTA V's collision materials.
- A collision material written to XML as its hash number was another
  material when read back.

## [2.3.5] - 2026-11-04

### Added
- Game Files' right click exports a file, or a folder's files: **Export as
  XML** (textures as DDS files next to it), **Extract** (as a RedM resource
  streams it) or **Extract uncompressed**, as in RedForge 1.x.
- **Edit > Export selection** writes the selected objects' models as XML,
  with the textures they use, as in 1.x.
- A **Blender** button in the map's toolbar, its dot lit while a Blender is
  connected; Ctrl+B and Hierarchy's right click send the selection to Blender
  too.

### Fixed
- The gizmo works right up close to an object: within half a metre of it,
  it couldn't be grabbed.
- With the selection's position out of view (a big model seen from close
  by), the Move gizmo is where the object was clicked.
- Edit in Blender works with Blender 5.2 (update the RedForge Bridge
  add-on in Blender): Sollumz RDR stopped halfway through importing tinted
  models, most props, and models with textures named like hexadecimal
  numbers (`d_93_c`); the add-on works around both. A model Sollumz can't
  import leaves nothing half made, and the other objects still come in.

## [2.3.0] - 2026-11-04

### Added
- Projects open from the file manager: double-click an `.rfproj` (the
  packages and the Windows installer make RedForge open them), or drop it on
  RedForge's window. A RedForge already running opens it, and asks before
  closing the project open, with **Save and open** when files have unsaved
  changes.
- `.rfproj` files have an icon of their own in the file manager: a page with
  "PROJ" and RedForge's anvil.
- Game files dropped on the window, or chosen with **Add files…**, join the
  project: ydr, ytd, ybn, ymap, ytyp… and their XML from Sollumz or CodeX,
  converted to the game's file next to the XML (as in RedForge 1.x), and
  folders. Files from another folder are copied into the project's, so the
  project keeps them (Project pane, More).
- Autosave: every two minutes, the files with unsaved changes are written
  aside; if RedForge closes before they're saved (a crash, a power cut),
  opening the project asks to restore them.
- Saving keeps the file it replaces as `<name>.bak`, and writes the new one
  whole or not at all: a crash while saving no longer leaves half a file.
- The RedM export takes out of the resource's `stream` the files an earlier
  export wrote that have left the project (and their ytyps' requests in
  `fxmanifest.lua`), so they don't load in game any more; other files there
  stay. It makes the project's `_manifest.ymf` again first.
- Files of the project that aren't on disk any more show with a warning,
  and a right click locates them; the export says which it left out.
- A File menu with the project's commands: New project, Open project
  (Ctrl+O), Open recent, Save all, Export RedM resource (Ctrl+E), Close
  project; and Quit with Ctrl+Q.
- RedForge remembers the window's size, the panes' layout, the world map
  (camera, hour, weather, speed, statistics, scripted maps) and the tools
  (selection mode, snaps, axes). The world map opens again where it was if
  it was open when quitting; with another project open, it looks at the
  project's ymaps.
- The welcome page has **Continue where you left off** (with the project
  and the nearest town), and the projects: New, Open and the recent ones.
- **Drop to ground** (End, or Edit > Drop to ground): the selection goes
  straight down, or up, onto the terrain, building or interior floor under
  its middle, as in RedForge 1.x.
- **Ctrl+Shift+D** copies the selection as far again as the last drag, for
  rows of fences and posts.
- Gizmo drags show how far they went by the mouse (metres, degrees or
  scale), and go ten times slower while Alt is held. With a move snap, the
  object lands on the world's grid along the axes it's dragged on.
- The arrows move the selection a step of the move snap, or 10 cm (Page Up
  and Down: up and down; Shift: ten steps).
- **Go to** takes coordinates (`x, y, z` or `vector3(...)`), and copies the
  camera's position as `vector3(...)`.
- Properties: **Place more** next to the archetype (the Place tool with
  it), and Copy, Paste, Reset rotation and Reset scale for the placement;
  Copy also puts the position on the clipboard as `vector3(...)`.
- Hierarchy: the object selected on the map is scrolled to, with its group
  opened; Shift selects a range; a right click looks at, hides, duplicates
  or deletes.
- New ymap: Enter makes it, and with no name it's named after the project
  (`<project>_01`).
- Cut (Ctrl+X); T for the Scale tool and P for Place.
- Messages over the bottom of the window, errors in red, wherever the focus
  is: saving and exporting say what they did, copying, pasting, duplicating
  and deleting say how many objects.
- The window's title shows the project, with a dot while files have
  unsaved changes.
- Help > Keyboard shortcuts (F1), and About RedForge with the version.
- View > Reset layout.
- The game folder is found by itself when Steam, the Rockstar Games Launcher,
  the Epic Games Launcher or Heroic on Linux installed the game.

### Changed
- The game folder's dialog, redone in the new look: centred, with the
  installs found, and **Cancel** when it's opened from File > Game folder….
- Over the map, only the camera's position shows; the statistics are in the
  gear menu (**Statistics**).
- A world map with nothing moving or loading draws a few frames a second
  instead of all the time, so the GPU and its fans rest.
- Game Files' search runs aside (typing no longer stutters over half a
  million paths), finds the files with all the words typed (`coffin ydr`),
  and shows the archive each one is in.
- Opening another map no longer loads the sky's and the water's textures
  again.
- The Project pane's list of files takes the pane's height (it had room for
  two or three), and the Hierarchy under it starts a little shorter.
- A notch of the wheel changes the speed by about 30% (it took 30 notches
  to go from 500 to 10 m/s), and the speed shows for a moment.

### Fixed
- Quitting with unsaved changes (File > Quit, the window's close button,
  Alt+F4) asks first, listing the files, with **Save and quit**; so does
  changing the game folder. The project open goes on in the new game folder,
  and choosing the same folder again doesn't load the game again.
- **Export when saving** works as soon as a project opens; it used to wait
  until the RedM resource section had been opened once.
- **New…** with a project open makes a new, empty project in place of it,
  asking about unsaved changes first; it used to copy the open project, its
  RedM resource included.
- The ymap new things go in ("Place in") is kept in the project and chosen
  again when it opens; a project with one ymap has it chosen. Placing or
  pasting without one says where to choose it.
- Game Files' **Copy path** copies the whole path, not only the file's name.
- Typing a value in Properties is one undo step when it's done (Enter, or a
  click elsewhere); it used to be one per key, moving the object at each.
  Hash and 64-bit fields (the entity's `id`) can be typed in, and whole
  numbers stay in their range (dragging below 0 gave 255).
- Dragging copies (Shift) is one undo step, **Copy and move**; when the
  copies can't be made the originals don't move. Esc during a drag puts
  everything back.
- Q and E turn what's placed at the same speed at any frame rate (45° a
  second, 180° with Shift), or by the angle snap's step a press.
- Opening the world map or a file that's open already brings its tab to the
  front, instead of opening it again (two world maps streamed twice).
- The towns on the welcome page open the map there.
- Ctrl+C and Ctrl+V copy and paste objects; they only worked from the Edit
  menu. Undo, redo, duplicate, delete and Home work wherever the focus is,
  not only with the mouse over the map (but while typing in a field).
- Esc clears selected helpers too, and an archetype error in Properties goes
  when the selection changes.
- **Remove from project** asks first when the file has unsaved changes.
- Props no longer show a plain shell over themselves (white, or another flat
  colour): the models a drawable has only for shadows (`cpv_only` shells,
  whose render mask leaves out the main view) are left out of the view, as
  in RedForge 1.x.
- Props show their HD textures: textures come from the model's texture
  dictionaries first, the HD ones (`+hi`, `+hifr`, `+hidr`) before the
  smaller copies inside the model, as in 1.x.
- Decals on walls and floors no longer flicker as the camera moves:
  positions are worked out relative to the camera, as in 1.x (far from the
  world's origin they lost a tenth of a millimetre), and the map draws its
  objects in the same order every frame.
- The window opens at least 800×500; one the compositor remembered tiny gets
  a usable size.
- The weather can be chosen again: its list closed the hour's menu instead
  of opening. It's a submenu now.
- Properties' flags stay open while ticking them; each tick closed the list.
- View's Project, Hierarchy and Game Files bring the pane to the front; one
  that was closed goes back beside the others, instead of in a new column
  every time.

## [2.1.0] - 2026-11-03

### Added
- A new look, **Islands**: the panels as rounded islands over a darker
  background, neutral greys with an orange accent, the Fira Sans font built
  in, and icons throughout.
- A welcome page with an **Open the world map** button and the towns to start
  at.
- A short tour of the panels the first time RedForge opens (**Help > Tour of
  the panels** shows it again).
- Linux packages for every distro: a `.deb` (Debian, Ubuntu), an AppImage and
  a Flatpak, built to run on glibc 2.31 and newer. The AUR packages
  (`redforge-bin`, `redforge-git`) are ready for when the AUR takes new
  accounts again.
- A code of conduct and issue forms for bugs, file format problems and
  feature requests.

### Changed
- The map's toolbar is compact groups of icons, their names on hover; the hour
  and the weather are one button, the speed, exposure and scripted maps a
  settings menu.
- Project and Hierarchy list their files and objects with icons, the selected
  one marked in the accent, under headers that say how many.
- The menus are in the usual order: File, Edit, View, Help.
- The README, rewritten: installing, getting started, the controls, the
  selection modes.

### Fixed
- Arrows in some labels showed as boxes.

## [2.0.0] - 2026-11-03

RedForge rewritten from scratch in Rust: one app for **Windows and Linux**,
compatible with 1.x projects, settings and the Blender add-on.

### Added
- **Linux**, native (Vulkan), with an Arch package; on Windows, an MSI
  installer.
- **Graphics closer to the game**: rivers and lakes with flow, foam and depth,
  the sun, moon and stars in the sky, glows only at their hours, fire, glass,
  translucent leaves and grass, peds' skin.
- **Instanced trees, bushes, rocks and grass** shown, and selectable and
  editable like entities.
- **Selection modes** in the toolbar: Light, Physics, Navmesh, Skeleton,
  Occlusion, Interior Instance, Time Cycle Modifier, Ambient Mask Volume,
  Scenario, each with its helpers to hover, select and inspect.
- **Placing**: the object's model shows under the mouse where it will go.
- **Project pane** grouped by file type, with search; a ymap picked there opens
  in the Hierarchy; the last project reopens at start.
- `rf`, a command-line tool: extract the archives, check them, and convert game
  files to and from XML.

### Kept from 1.x
- Every file format read, written and converted to XML exactly as 1.x does
  (Sollumz/CodeX XML).
- World streaming, entities and interiors editing with undo, the Objects pane
  with thumbnails, `.rfproj` projects, the RedM resource export, the Blender
  bridge, the model, texture, animation and hex viewers.
- Shape outlines on hover and selection, the hovered name under the mouse, the
  loading screen.

### Known issues
- Helpers (occluders, rooms, collisions…) can be seen and selected but not
  moved or edited yet.
- Tree impostors past the last LOD are missing.
- The Windows installer still has to be tried on Windows.

[Unreleased]: https://github.com/Simomagy/RedForge/compare/v2.4.0...HEAD
[2.4.0]: https://github.com/Simomagy/RedForge/compare/v2.3.6...v2.4.0
[2.3.6]: https://github.com/Simomagy/RedForge/compare/v2.3.5...v2.3.6
[2.3.5]: https://github.com/Simomagy/RedForge/compare/v2.3.0...v2.3.5
[2.3.0]: https://github.com/Simomagy/RedForge/compare/v2.1.0...v2.3.0
[2.1.0]: https://github.com/Simomagy/RedForge/compare/v2.0.0...v2.1.0
[2.0.0]: https://github.com/Simomagy/RedForge/releases/tag/v2.0.0
