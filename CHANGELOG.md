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
- `.rfproj` files have an icon of their own in the file manager: a page with
  "PROJ" and RedForge's anvil.
- The game folder is found by itself when Steam, the Rockstar Games Launcher,
  the Epic Games Launcher or Heroic installed the game.

### Changed
- The game folder's dialog, redone in the new look: centred, with the
  installs found, and **Cancel** when it's opened from File > Game folder….

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
