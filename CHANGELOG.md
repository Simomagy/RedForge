# Changelog

All notable changes to RedForge are written here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): the major number
for changes that break projects or files, the minor for new features, the
patch for fixes. Each version's section pastes as is into its GitHub release.

## [Unreleased]

### Added
- Projects open from the file manager: double-click an `.rfproj` (the
  packages and the Windows installer make RedForge open them), or drop it on
  RedForge's window. A RedForge already running opens it, and asks before
  closing the project open, with **Save and open** when files have unsaved
  changes.
- Game files dropped on the window, or chosen with **Add files…**, join the
  project: ydr, ytd, ybn, ymap, ytyp… and their XML from Sollumz or CodeX,
  converted to the game's file next to the XML (as in RedForge 1.x), and
  folders. Files from another folder are copied into the project's, so the
  project keeps them (Project pane, More).
- `.rfproj` files have an icon of their own in the file manager: a page with
  "PROJ" and RedForge's anvil.
- A File menu with the project's commands: New project, Open project
  (Ctrl+O), Open recent, Save all, Export RedM resource (Ctrl+E), Close
  project; and Quit with Ctrl+Q.
- Help > Keyboard shortcuts (F1), and About RedForge with the version.
- Messages over the bottom of the window, errors in red, wherever the focus
  is: saving and exporting say what they did, copying, pasting, duplicating
  and deleting say how many objects.
- The window's title shows the project, with a dot while files have
  unsaved changes.
- Cut (Ctrl+X); T for the Scale tool and P for Place.
- RedForge remembers the window's size, the panes' layout, the world map
  (camera, hour, weather, speed, statistics, scripted maps) and the tools
  (selection mode, snaps, axes). The world map opens again where it was if
  it was open when quitting; with another project open, it looks at the
  project's ymaps.
- The welcome page has **Continue where you left off** (with the project
  and the nearest town), and the projects: New, Open and the recent ones.
- View > Reset layout.
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
- The game folder is found by itself when Steam, the Rockstar Games Launcher,
  the Epic Games Launcher or Heroic on Linux installed the game.

### Changed
- The game folder's dialog, redone in the new look: centred, with the
  installs found, and **Cancel** when it's opened from File > Game folder….
- Over the map, only the camera's position shows; the statistics are in the
  gear menu (**Statistics**).
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

[Unreleased]: https://github.com/Simomagy/RedForge/compare/v2.1.0...HEAD
[2.1.0]: https://github.com/Simomagy/RedForge/compare/v2.0.0...v2.1.0
[2.0.0]: https://github.com/Simomagy/RedForge/releases/tag/v2.0.0
