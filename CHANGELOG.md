# Changelog

## RedForge 2.0

RedForge rewritten from scratch in Rust: one app for **Windows and Linux**,
compatible with 1.x projects, settings and the Blender add-on.

### New
- **Linux**, native (Vulkan), with an Arch package; on Windows, an MSI installer.
- **Graphics closer to the game**: rivers and lakes with flow, foam and depth,
  the sun, moon and stars in the sky, glows only at their hours, fire, glass,
  translucent leaves and grass, peds' skin.
- **Instanced trees, bushes, rocks and grass** shown, and selectable and editable
  like entities.
- **Selection modes** in the toolbar: Light, Physics, Navmesh, Skeleton,
  Occlusion, Interior Instance, Time Cycle Modifier, Ambient Mask Volume,
  Scenario, each with its helpers to hover, select and inspect.
- **Placing**: the object's model shows under the mouse where it will go.
- **Project pane** grouped by file type, with search; a ymap picked there opens
  in the Hierarchy; the last project reopens at start.
- `rf`, a command-line tool: extract the archives, check them, and convert game
  files to and from XML.

### Same as 1.x
- Every file format read, written and converted to XML exactly as 1.x does
  (Sollumz/CodeX XML).
- World streaming, entities and interiors editing with undo, Objects pane with
  thumbnails, `.rfproj` projects, RedM resource export, Blender bridge, model,
  texture, animation and hex viewers.
- Shape outlines on hover and selection, the hovered name under the mouse, the
  loading screen.

### Not yet
- Helpers (occluders, rooms, collisions…) can be seen and selected but not
  moved or edited yet.
- Tree impostors past the last LOD.
- The Windows installer still has to be tried on Windows.
