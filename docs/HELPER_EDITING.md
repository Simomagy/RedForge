# Editing helpers: the study

The selection modes show helpers (navmeshes, occluders, ambient mask volumes,
scenario points…) that can be hovered and selected, but not edited. This is
what editing them takes, type by type, studied on the game's files, the Rust
code and the C# RedForge (November 2026). "Verified" means read in the code or
run on game files; the rest is inferred and marked so.

## What every type needs first

The editor is built around entities in ymaps and ytyps:

- `DocKey` is `Map` or `Types`, and every document's root is a `DataBag`
  (`rf-editor/src/lib.rs:119`). A ynv is a `Navmesh` struct, not a bag.
- The selection and undo steps hold `EntityId`s only (`lib.rs:141`). Selected
  helpers are the app's `Tools::helper_selection`, which nothing in the editor
  knows about (`map/mod.rs:1104`).
- The gizmo, nudges and drops work on `(EntityId, Placement)`
  (`map/tools.rs:102`).
- Properties shows a helper's `PartInfo` as text (`properties.rs:69`).
- The navmesh, scenario and AMV sources read the game's files (or the
  project's, through `with_project`), never a document being edited.

So, before any type:

1. **Items**: a selection of entities *or* helpers, `Item::Helper(HelperRef)`
   with the document, the kind and the index; undo steps keep it, and edits
   that move indices (delete, insert, re-sort) remap it, as deletes remap
   `EntityId`s now.
2. **Documents of other kinds**: `DocKey::Navmesh`, `DocKey::Scenario`,
   `DocKey::Amv`, with roots that aren't bags (an enum of roots), each with its
   reader, writer and project file.
3. **The helpers draw the documents**: each source asks the editor for an
   edited document before reading the file, and makes its parts again when it
   changes (the ymap helpers already do, through `World::set_map`).
4. **The gizmo on helpers**: a placement per item with its limits (only about
   Z, size instead of scale, no scale), previews while dragging, one step on
   release.
5. **Properties that edit**: typed fields per kind (numbers, hashes with
   pickers, flags) instead of text.

The C# had the same idea: `EditablePart.SetGizmoTransform` then
`ApplyTransforms` writes the part back to its file's data
(`RDR2Map.cs:3732-4342`), for interior rooms, portals and time cycle spheres,
box occluders, ymap time cycle boxes and AMV items. It never edited
navmeshes, occlude models (but through its mesh editor) or scenario points.

## Occlusion (box occluders, occlude models)

- **Where**: `CMapData.boxOccluders` and `occludeModels`, in ymaps (the
  `*_occ.ymap`s in `levels_6.rpf/.../terrain/occlusion/` have models; the
  `area_lod_combine_metadata` ymaps have boxes). Verified.
- **Encoding** (verified, all floats, no quantising as in GTA V):
  - A box: `CenterXYZ_CosZ` and `SizeXYZ_SinZ`, the whole size, turned about
    Z by `atan2(SinZ, CosZ)`.
  - A model: `bmin`/`bmax` (the vertices' box), `verts` (`numVertsInBytes/12`
    world-space f32 vertices, then u8 indices), `dataSize = 12v + 3t`,
    `numTris = 0x8000 | t`, `flags = 0`; at most 255 vertices.
- **Bug**: `rf-world/src/helpers.rs:145` reads the angle as the C# does,
  `atan2(CosZ, SinZ)`, so boxes show at 90° − θ. The game's extents and
  `rf-editor/src/save.rs` agree with `atan2(SinZ, CosZ)`. Verified.
- **Saving**: ymaps are already edited and saved (`MapDoc`), occluders pass
  through as read, and `save::update_map` already counts them in the extents.
  Only the editor's streaming box (`World::set_map`, `edit.rs:54`) grows from
  entities alone: occluders moved out of it, or in a ymap with only occluders,
  wouldn't show.
- **RedM**: ymaps stream as they do today. Nothing to test.
- **Operations**:
  1. Box occluders: move, turn about Z, resize, delete, new (from the
     selected entity's box). Cheap.
  2. Occlude models: move, turn or scale the whole model (written into its
     vertices, box recomputed), delete; a model from a box (8 vertices, 12
     triangles, any rotation).
  3. Editing a model's vertices: a mesh editor. Costly.
- **Time cycle modifier boxes** are in the same ymaps, read and counted in
  the extents already: moving and resizing them is almost free with this
  work. Their `transform` (43 of 377 turned 90°) should stay untouched until
  whether the extents are local or world is known.
- **Interior rooms, portals and spheres** are in ytyps' MLO archetypes, shared
  by every instance (`TypesDoc`): separate work.

## Navmeshes (ynv)

- **Where**: `navmeshes0-9.rpf`, a grid of 215 × 215 cells of 150 m from
  (-15900, -15900), named `navmesh[3i][3j]`, area id `i + 215 j`; vehicles'
  navmeshes and swaps (`navmeshes_swaps.rpf`) besides. Verified.
- **Format** (`rf-formats/src/formats/ynv/mod.rs`):
  - Vertices: u16 × 3, quantised in the navmesh's box (XY the cell, Z fitted
    to the vertices); outside the box they wrap around silently.
  - Polygons (56 bytes): their range of indices, 36 flag bits, audio, water
    depth, a box in half metres, a centroid (u8), their list part, their links.
  - Edges: one per corner, the polygon across, the space around the vertex,
    cover, external and drop flags.
  - A quadtree of polygon ids; special links (ladders, jumps, climbs) with
    quantised ends; no cover points in RDR2.
- **Neighbours** (verified): no edge points into another cell. Each border
  has 93–126 two-corner "stitch" polygons matching the neighbour's, and each
  cell has 31–142 special links to polygon *indices* in its neighbours. So
  renumbering a cell's polygons breaks its neighbours' links, and moving its
  border vertices breaks the stitch.
- **Writing**: `ynv::write` and the XML builder (`build.rs`) exist; a read and
  write loses nothing (19/19 files read back equal, 40/40 XML equal to the
  C#'s), though not byte for byte. The builder rebuilds the lists, the
  quadtree and the polygons' boxes, but not adjacency, the box, the centroids,
  or polygon ids.
- **C#**: showed and converted them, never edited them.
- **App**: one part per render chunk of up to 65280 vertices, so a polygon
  can't be picked today; the parsed navmesh is thrown away after making the
  meshes.
- **RedM**: a ynv of the same name in `stream` should replace the game's, as
  in FiveM. **Not tested.**
- **Operations**, cheapest first:
  1. Polygon flags and attributes (switched off for peds, audio, water…):
     nothing to rebuild. Needs picking polygons, and painting them.
  2. Moving vertices inside the cell: the box's Z, every vertex and link
     quantised again, the polygons' boxes and centroids, the quadtree; border
     vertices stay; the slope flags go stale.
  3. Adding or deleting polygons: adjacency, every id, the neighbours' links.
     Adding at the end and "deleting" by switching off avoids renumbering.
  4. Special links.
  5. Making navmeshes from the collisions: a navmesh builder. Out of reach.

## Scenario points

- **Where** (verified): `sp_manifest.ymt` (RBF) lists 621 regions with their
  boxes and ~2100 groups; `sp_generated_manifest.ymt` (PSO). The regions are
  RSC8 meta ymts, read by `Meta::read`.
- **A region**: `Points.MyPoints` (type, model set, group, interior and
  required IMAP as plain hashes, no lookup tables; bytes for probability,
  times, radius…; 64-bit flags; position and heading), entity overrides (by
  prop), the chaining graph (nodes, edges, chains, a spatial grid of edges),
  an accel grid (points sorted by cell, a cumulative index per cell),
  clusters, species filters, conditional anims.
- **Links**: chain nodes and cluster locators match points **by position**,
  not by index. Nothing indexes points.
- **Writing**: `Meta::save` writes them; all 710 regions read back equal (a
  move and a delete too), though not byte for byte (deflate instead of
  Oodle). Verified.
- **Recomputed on an edit**: the points' order and accel grid, the chaining
  graph's box; the node or locator on a moved point moves with it. A chained
  point's delete removes its node and edges and remaps the chains. Points
  must stay in the region's manifest box (the manifest can't be replaced
  safely). A new type resets `DisabledConditionalAnims`.
- **C#**: showed MyPoints only, never edited them.
- **App**: `HelperKey` is the region's stem and the index in MyPoints; stable,
  but edits that re-sort need a remap.
- **RedM**: a region of the same name in `stream` should replace the game's.
  A new region needs a manifest and a `data_file` RedM may not have.
  **Not tested.**
- **Operations**: move and turn points; edit their fields (pickers for types,
  model sets, groups; flags once their bits are checked: some come in pairs);
  add (copy) and delete; then chains; new regions last.

## Ambient mask volumes

- **What** (verified): `amv_zone_<n>.ymt` (PSO) in `amvplaced.rpf`, each with
  its `amv_zone_<n>.ytd`: every item is a box (position, euler rotation, half
  size), falloffs, clip planes, blending, flags, a uuid, and two 3D textures
  in the ytd named after the uuid, baked offline (ambient occlusion,
  inferred). 179 zones in the base game, 183 renumbered in update_4.
- **Writing**: the PSO writer reads back equal on all 183 zones (verified),
  but a bag as `load_doc` reads it must be bound to the built-in schema first
  (it writes no root otherwise); PSIG, STDN and CHKS are left out.
- **C#**: moved, added and deleted items, but its writer fails on every zone:
  it never saved one.
- **What makes sense**: deleting, switching off (`enabled`) and the falloff,
  blend and flag fields are safe; moving or resizing only with the building
  the volume was baked for; duplicating needs a new uuid and copies of the
  textures. New volumes can't be baked.
- **RedM**: same-name replacement in `stream` is likely, but tied to the
  game's build (the zones are renumbered by updates). **Not tested.**
- **Use**: niche: mostly the dark ambient left where a game building was
  removed or moved.

## Tests in RedM first

Ymaps are known to stream in RedM; the other three aren't. Each is cheap to
test with the `rf` tool before building on it:

- a navmesh near Valentine with a few polygons switched off for peds;
- a Valentine scenario region with points moved and deleted;
- an AMV zone with its items switched off.
