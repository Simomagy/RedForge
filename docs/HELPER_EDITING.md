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
- **Done** (`rf-editor/src/occlusion.rs`, the app's `map/tools.rs`): boxes
  and models move, turn, scale and are deleted, with undo; the selection
  stays the app's (`Tools::helper_selection`), dropped when undo or redo
  changes how many occluders its ymaps have. New occluders, copies and a
  model's vertices are still to do.
- **Bug, fixed**: the angle was read as the C# does, `atan2(CosZ, SinZ)`, so
  boxes showed at 90° − θ. The game's extents and `rf-editor/src/save.rs`
  agree with `atan2(SinZ, CosZ)`. Verified.
- **Saving**: ymaps are already edited and saved (`MapDoc`), occluders pass
  through as read, and `save::update_map` already counts them in the extents.
  The editor's streaming box (`World::set_map`) grows to take in the
  occluders too.
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
- **App**: one part per render chunk of up to 65280 vertices; a polygon is
  picked on the CPU from the point the GPU pick hit (`Flat::polygon_at`), and
  the selection and hover are drawn over the navmesh.
- **RedM** (tested 2026-11-27, issue #2): a ynv of the same name in a
  resource's `stream` replaces the game's. The client log's
  `ResourceCache::AddEntry: Saved cache:v1:<sha1>` is the file's SHA-1,
  fetched when the game first needs the cell. A file written by RedForge
  loads, and the cell still joins its neighbours. A changed ynv needs a
  reconnect: `restart` of the resource while connected ended in
  `RAGE error: 0x1F045E58:441` from `RageHddReader` and a crash.
  - `SwitchedOffForPeds` alone does not stop the paths scripts ask for
    (`NAVMESH_REQUEST_PATH` went straight through); its effect on ambient peds
    is untested.
  - Cutting every edge between a set of polygons and the rest (both ways,
    as the mesh's border: area 65535, polygon 0x7FFF) does: the path went
    around, and the ped too.
- **Done** (`rf-editor/src/navmesh.rs`, 2.7): navmeshes as documents
  (`DocKey::Navmesh`, undo, project, autosave), polygons' flags, audio and
  water depth, **Block** (the island above, plus switched off for peds) and
  **Unblock** (the game's edges back).
  - Block also takes the special links between the island and the rest out of
    the polygons' lists: each polygon lists the links it starts or ends
    (`link_id`, `link_count` into `special_links_inds`), so its range is
    reordered with them last and its count lowered; nothing is renumbered.
    Each link in use is listed by both its polygons; its pair, the way back,
    by none. A link's end in another cell is quantised in that cell's box.
    The island tested in game had 28 such entries left (the test's path used
    none); cutting them is not tested in game yet. Edits are in place
  (`NavmeshList::get_mut`): the lists, vertices and quadtree stay as read.
- **Operations**, cheapest first:
  1. Polygon flags and attributes (switched off for peds, audio, water…):
     nothing to rebuild. Done, with blocking by cutting edges.
  2. Moving vertices inside the cell: the box's Z, every vertex and link
     quantised again, the polygons' boxes and centroids, the quadtree; border
     vertices stay; the slope flags go stale.
  3. Adding or deleting polygons: adjacency, every id, the neighbours' links.
     Adding at the end and "deleting" by switching off avoids renumbering.
  4. Special links.
  5. Making navmeshes from the collisions: a navmesh builder. Out of reach.

## Navmesh generation (2.8)

The study (issue #6, `cargo run --release -p rf-editor --example
navgen_study [grid|fine|sizes|edges|flags]`) generated navmesh with Recast
(`rerecast` 1.0, MIT OR Apache-2.0) from the static collision of 8 places,
towns and country (Valentine, Rhodes, Strawberry, Blackwater, Saint Denis,
fields and slopes around), 80 m each, and compared it with the game's on a
0.5 m grid. Verified:

- **Collision**: the game's triangles face up for floors (as stored);
  composite children with `CF_MAP_TYPE_MOVER` or `CF_MAP_TYPE_HORSE` are the
  ones walked on (`hi@` and the horse avoidance ybns have none);
  `FLAG_NO_NAVMESH` materials are left out. A child with no type flags at
  all takes its instance's, which peds collide with: no game model has one
  (Valentine, Tumbleweed), but a custom building's own collision can (the
  Tumbleweed school's stairs and railings were missed without it). A custom
  interior isn't in the game data's interiors: its collision is the ybn of
  its archetype's name.
- **Settings** (`navgen::Settings::default`): agent radius 0.25-0.3 m fits
  best (91% of the walkable ground the same, recall 0.96-0.99); the step
  (0.35-0.55 m) and the slope (50-60°) hardly matter. Strawberry (wooden
  walkways, stairs) is the worst at 0.75.
- **Polygons**: the game's are 1-3.5 m² on average, with up to 10 corners,
  counter-clockwise. In the open they're a grid of 3.97 m squares; with an
  outline error of 0.3 Recast's match the game's sizes in towns, and a cut on
  a 4 m grid from the cell's corner gives the open ground's.
- **Edges**: "space around vertex" is the distance to the nearest edge with
  nothing across, in steps of about 0.24 m (4 ≈ 0.95 m, 8 ≈ 1.9 m, up to 31).
  "Space beyond edge" and the adjacency type are 0 everywhere looked at.
- **Centroids**: `centroid_x/y` are the corners' middle in the polygon's box
  (in half metres, rounded outwards), in 256ths.
- **Flags**: `Small` is every polygon under 2 m²; `Large` isn't used in
  Valentine. `SteepSlope` has a median slope of 47° (95% over 42°), but many
  steeper polygons (steps) lack it. `Stairs` is on a `FLAG_STAIRS` material
  56% of the time. Pavement, Road, Boardwalk, Vegetation, Water,
  NetworkSpawnCandidate and the shelters are the game's choice by place, not
  by material (Valentine's deep mud is Pavement and Road): generated polygons
  take them from the game's polygon under them. `Door` is on the polygons
  in an interior's doorways: of the game's polygons within 0.25 m of a
  portal (seen from above, at its heights), 80-99% have it, and they are
  73-85% of those that do (Valentine, Tumbleweed, Rhodes); door entities
  don't say it (Rhodes' Door polygons are 24 m from one on average).
- **Done** (`rf-editor/src/navgen/`): `collision` (the static ybns, the
  interiors' and the models' of the entities around, doors and gates left
  out), `generate` (Recast, Y-up: x, z, y), `stitch` (the region's polygons
  replaced, the ones crossing it cut into convex pieces, the sides' vertices
  shared, joins by position and two-way only, links put on the polygon under
  their end or the nearest within 1 m), and `Editor::generate_navmesh` (the
  4 m grid, the ground's height under each corner, the flags, `Door` within
  0.25 m of a portal, the next cells' links into the cell, one undo step). `rf navmesh-make` does it from the
  command line. Areas and slopes are computed from the first corner in f64:
  at Tumbleweed (x -5600) f32 products were 2 m² off.
- **Tested offline**: Valentine's main street (20 m box: 258 polygons out, 512
  in, still walkable across, 2 links of 316 lost, the next cell's 61 links
  moved); the Tumbleweed school, a custom interior (28 m box: its 436 floor
  polygons, `Interior`, all reachable from outside through the door).
- **Cells' borders** (verified in Valentine's and Tumbleweed's cells): no
  edge of the game's navmeshes points into another cell (every edge's area is
  its own or none). Cells are joined when the game loads them, by their
  border polygons: ~150 per cell, flagged `ZeroAreaStitchPolyDLC` and
  `LiesAlongEdgeOfMesh`, flat along a side of the cell (the cells' boxes are
  the 150 m grid exactly); 2 corners for most, up to 11. Each has exactly
  one edge facing out of the cell (left of it is outside; none, flag 8
  `EXTERNAL_EDGE`), a stretch of the ~3.97 m grid the next cell's border
  polygon has too, end for end. Its other edges come back along the stretch
  through the corners of its cell's polygons there, each joined to the
  polygon whose edge goes **the same way round** (not reversed, as between
  two polygons); a gap is none with flag 8. A box that reaches a side makes
  the border polygons it reaches again along the same stretches, with the
  corners of the polygons there now (`stitch::replace_region`); one that
  crosses a border is made with one Recast run and cut per cell
  (`Editor::make_navmesh`, sides within 1.5 m of a cell's side go to it).
  Offline (`navmesh_made_across_cells`): Valentine's x = -300 border, 10
  border polygons made again, stretches the same in both cells, the street
  walkable from one cell into the other through them.
- **Preview**: `Editor::make_navmesh` makes without putting in
  (`MadeNavmesh`), the map draws it in place of the navmeshes it changes,
  and `apply_made_navmesh` puts it in as one undo step, unless one of them
  changed since.
- **Tested in game** (RedM, `rf_navtest_school`): a ped walks from outside
  into the school's classroom through its south-east door, on the path the
  game finds (24 m, 22 s, no stop). The first try walked into the stairs'
  railing: the building's own collision (`pry_schoolhouse.ydr`, children
  without type flags) had been left out. Then a ped from outside the box
  (`/navborder`) and a horse around the school, crossing the box twice
  (`/navhorse`), with `Door` flags in the doorways: no stop.

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
