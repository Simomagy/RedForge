"""RedForge Bridge: edit the objects of RedForge's map in Blender, and send the changes back.

"Edit in Blender" in RedForge (or Edit RedForge Selection here) brings the selected objects in, with their models through
Sollumz RDR. Send Changes to RedForge sends back the models that were edited (through Sollumz: RedForge puts them in its
project, so the map shows them straight away) and where objects were moved, turned or scaled to.

The map's coordinates are thousands of metres from the origin, where Blender's precision isn't enough to edit, so the
objects come in around a working origin kept with the scene: Blender shows them near 0,0,0 and positions go back to
RedForge with the origin added.
"""

import importlib
import math
import os
import re
import shutil
import tempfile
import time
from pathlib import Path

import bpy
from bpy.app.handlers import persistent
from bpy.props import BoolProperty, IntProperty, StringProperty
from mathutils import Matrix, Quaternion, Vector

from . import link

COLLECTION = "RedForge"
# on the root of each object from RedForge
FILE_KEY = "redforge_file"  # the model's game file, eg. p_lantern05x.ydr (objects using the same one share its meshes)
OBJECT_KEY = "redforge_object"  # RedForge's id for the map object
WORLD_KEY = "redforge_world"  # where the object is on the map (to find it again after RedForge restarted)
SENT_KEY = "redforge_sent"  # (on every part too) its location, rotation and scale here when RedForge last had them
# on the scene
ORIGIN_KEY = "redforge_origin"  # the map position of Blender's 0,0,0
EXPORTS = Path(os.environ.get("LOCALAPPDATA") or tempfile.gettempdir()) / "RedForge" / "Blender" / "edit"  # where RedForge puts the models it sends here, a folder each with its textures (BlenderBridge.ExportFolder)
RECENTRE_DISTANCE = 500.0  # metres: objects coming in farther than this from the origin get a new one around them
YTYPS_KEY = "redforge_ytyps"  # the names of the scene's Sollumz ytyps that Send YTYPs sends (ticked in the panel)
CHANGED_KEY = "redforge_changed"  # the models edited since they were last sent (kept when the file is saved)

SOLLUMZ_MODELS = {"sollumz_drawable": "ydr", "sollumz_drawable_dictionary": "ydd", "sollumz_fragment": "yft"}
EXPORTERS = {  # model type -> Sollumz exporter
    "ydr": ("ydr.ydrexport", "export_ydr"),
    "ydd": ("ydd.yddexport", "export_ydd"),
    "yft": ("yft.yftexport", "export_yft"),
}
_changed = set()  # model files edited since they were last sent
_quiet_until = 0.0  # (importing makes geometry updates too, which aren't edits)
_was_connected = False


def prefs(context=None) -> "RedForgePreferences":
    return (context or bpy.context).preferences.addons[__package__].preferences


def sollumz(module: str):
    """A module of the enabled Sollumz RDR add-on (its package name depends on where it's installed)."""
    for name in bpy.context.preferences.addons.keys():
        if name.rsplit(".", 1)[-1] == "sollumz_rdr_dev":
            return importlib.import_module(f"{name}.{module}")
    raise RuntimeError("Sollumz RDR isn't enabled: RedForge Bridge imports and exports models through it")


class _Setting:
    """Sets a Sollumz setting for a while, and puts it back."""

    def __init__(self, settings, name, value):
        self.settings, self.name, self.value = settings, name, value

    def __enter__(self):
        self.old = getattr(self.settings, self.name)
        setattr(self.settings, self.name, self.value)

    def __exit__(self, *_):
        setattr(self.settings, self.name, self.old)


def model_root(obj):
    while obj is not None and FILE_KEY not in obj:
        obj = obj.parent
    return obj


def hierarchy(root):
    return [root, *root.children_recursive]


def redforge_roots():
    """The objects from RedForge, and the new models sent to it."""
    return [obj for obj in bpy.context.scene.objects if FILE_KEY in obj]


def sollumz_root(obj):
    """The Sollumz model an object is part of: the drawable, dictionary or fragment at its top."""
    root = None
    while obj is not None:
        if getattr(obj, "sollum_type", "") in SOLLUMZ_MODELS:
            root = obj
        obj = obj.parent
    return root


def model_name(name: str) -> str:
    """The file name Sollumz's Export gives a model: lowercase, without Blender's .001 suffix."""
    return re.sub(r"\.\d+$", "", name.lower())


def redraw():
    for window in bpy.context.window_manager.windows:
        for area in window.screen.areas:
            if area.type == "VIEW_3D":
                area.tag_redraw()


def transform(obj):
    location, rotation, scale = obj.matrix_basis.decompose()
    return [*location, *rotation, *scale]


def sent_matrix(obj) -> Matrix:
    was = obj[SENT_KEY]
    return Matrix.LocRotScale(Vector(was[0:3]), Quaternion(was[3:7]), Vector(was[7:10]))


def differs(obj) -> bool:
    """Moved, turned or scaled since RedForge last had it."""
    now, was = transform(obj), obj.get(SENT_KEY)
    if was is None:
        return False
    return ((Vector(now[0:3]) - Vector(was[0:3])).length > 0.0001
            or abs(Quaternion(now[3:7]).dot(Quaternion(was[3:7]))) < 0.9999999
            or (Vector(now[7:10]) - Vector(was[7:10])).length > 0.00001)


def mark_sent(objs):
    for obj in objs:
        obj[SENT_KEY] = transform(obj)


def moved(root) -> bool:
    """The map object was moved: its root, or the parts right under it (clicking the model in the viewport selects those)."""
    return (OBJECT_KEY in root) and (differs(root) or any(differs(child) for child in root.children))


def model_edited(root) -> bool:
    """Parts deeper in the model were moved (eg. collision boxes), which changes the model. For a model that isn't
    a map object (made here), moving any part of it does."""
    return any(differs(obj) for obj in root.children_recursive if (obj.parent is not root) or (OBJECT_KEY not in root))


def take_part_moves(root):
    """Parts right under the object moved instead of it: that's the object's move, so it goes there and they go back.
    Parts moved in different ways are an edit of the model instead."""
    parts = [child for child in root.children if differs(child)]
    if (not parts) or (OBJECT_KEY not in root):
        return
    # (the move in the object's space: part now = moved @ part before)
    moves = [p.matrix_parent_inverse @ p.matrix_basis @ sent_matrix(p).inverted() @ p.matrix_parent_inverse.inverted() for p in parts]
    if any(max(abs(a - b) for row_a, row_b in zip(m, moves[0]) for a, b in zip(row_a, row_b)) > 0.0001 for m in moves[1:]):
        _changed.add(root[FILE_KEY])
        return
    root.matrix_basis = root.matrix_basis @ moves[0]
    for part in parts:
        part.matrix_basis = sent_matrix(part)


# ---- RedForge -> Blender


def linked_copy(root, collection):
    """Another object with the same model: its meshes are shared, so an edit shows on all of them."""
    copies = {}
    for obj in hierarchy(root):
        copy = obj.copy()
        collection.objects.link(copy)
        copies[obj] = copy
    for obj, copy in copies.items():
        copy.parent = copies.get(obj.parent)
        copy.matrix_parent_inverse = obj.matrix_parent_inverse.copy()
    return copies[root]


def import_model(xml: Path, collection):
    before = set(bpy.data.objects)
    with _Setting(sollumz("sollumz_preferences").get_import_settings(), "import_as_asset", False):
        bpy.ops.sollumz.import_assets(directory=str(xml.parent), files=[{"name": xml.name}])
    root = next((obj for obj in bpy.data.objects if (obj not in before) and (obj.parent is None)), None)
    if root is not None:
        for obj in hierarchy(root):
            for other in list(obj.users_collection):
                other.objects.unlink(obj)
            collection.objects.link(obj)
    return root


def recentre(scene, placed, old, centre):
    """A new working origin in the middle of the objects coming in, so one alone is at 0,0,0. The map objects already
    here keep their map positions: they move by the difference, and so does what RedForge last had for them."""
    if old is not None:
        shift = [o - c for o, c in zip(old, centre)]
        for obj in placed:
            obj.location = [l + s for l, s in zip(obj.location, shift)]
            if SENT_KEY in obj:
                sent = list(obj[SENT_KEY])
                sent[0:3] = [v + s for v, s in zip(sent[0:3], shift)]
                obj[SENT_KEY] = sent
    scene[ORIGIN_KEY] = centre


def export_folder(image):
    """The model folder in RedForge's exports an image is loaded from, if it is."""
    if not image.filepath:
        return None
    try:
        return Path(bpy.path.abspath(image.filepath)).resolve().relative_to(EXPORTS.resolve()).parts[0]
    except (ValueError, OSError, IndexError):
        return None


def reload_pictures(folders):
    """Pictures of models RedForge just exported again: back if a cleanup took them (Sollumz won't send a model without them)."""
    names = {folder.name for folder in folders}
    for image in bpy.data.images:
        if (export_folder(image) in names) and (image.size[0] == 0):  # (no picture: its file was missing)
            image.reload()


def unused_exports():
    """RedForge's exported models the open .blend doesn't load anything from (nor has as objects), and their size."""
    if not EXPORTS.is_dir():
        return [], 0
    keep = {export_folder(image) for image in bpy.data.images} | {root[FILE_KEY] for root in redforge_roots()}
    folders = [d for d in EXPORTS.iterdir() if d.is_dir() and (d.name not in keep)]
    size = sum(f.stat().st_size for d in folders for f in d.rglob("*") if f.is_file())
    return folders, size


def clean_exports() -> str:
    folders, size = unused_exports()
    for folder in folders:
        shutil.rmtree(folder, ignore_errors=True)
    gone = sum(1 for folder in folders if not folder.exists())
    return f"Removed {gone} exported model(s), {size / 1048576:.1f} MB freed (the ones this .blend uses stay)"


def edit_selection(context) -> str:
    """Bring in RedForge's selected objects, around the working origin. Objects already here are just selected."""
    global _quiet_until
    result = link.request(prefs(context).port, "selection")
    if any("id" not in placement for placement in result["placements"]):
        raise link.LinkError("This RedForge is older than the add-on: rebuild or update RedForge")
    scene = context.scene
    collection = bpy.data.collections.get(COLLECTION)
    if collection is None:
        collection = bpy.data.collections.new(COLLECTION)
        scene.collection.children.link(collection)
    roots = redforge_roots()
    placed = [obj for obj in roots if OBJECT_KEY in obj]  # (map objects: the new models made here aren't anywhere on the map)
    positions = [placement["position"] for placement in result["placements"]]
    if positions:
        centre = [sum(axis) / len(positions) for axis in zip(*positions)]
        old = list(scene[ORIGIN_KEY]) if ORIGIN_KEY in scene else None
        if (old is None) or (not placed) or (math.dist(centre, old) > RECENTRE_DISTANCE):
            recentre(scene, placed, old, centre)
    origin = list(scene.get(ORIGIN_KEY, (0, 0, 0)))  # (sums with it are done in Python's doubles: Blender's vectors are single precision)
    xmls = {asset["file"]: Path(asset["xml"]) for asset in result["assets"]}

    selected, imported = [], 0
    for placement in result["placements"]:
        file, position = placement["file"], Vector(placement["position"])
        root = next((obj for obj in roots if obj.get(OBJECT_KEY) == placement["id"]), None) or next(
            (obj for obj in roots if (obj[FILE_KEY] == file) and ((Vector(obj.get(WORLD_KEY, (0, 0, 0))) - position).length < 0.01)), None)
        if (root is None) and (adopt := next((obj for obj in roots if (obj[FILE_KEY] == file) and (OBJECT_KEY not in obj)), None)):
            # (a model made here and sent, now placed in RedForge: this is that map object, not a copy of it)
            root = adopt
            root.location = [p - o for p, o in zip(placement["position"], origin)]
            root.rotation_mode = "QUATERNION"
            root.rotation_quaternion = placement["rotation"]
            root.scale = placement["scale"]
            mark_sent([root])  # (its parts keep what's still to send)
        if root is None:
            model = next((obj for obj in roots if obj[FILE_KEY] == file), None)
            if model is not None:
                root = linked_copy(model, collection)
            elif file in xmls:
                root = import_model(xmls[file], collection)
                imported += 1
            if root is None:
                continue
            root[FILE_KEY] = file
            root.location = [p - o for p, o in zip(placement["position"], origin)]
            root.rotation_mode = "QUATERNION"
            root.rotation_quaternion = placement["rotation"]
            root.scale = placement["scale"]
            mark_sent(hierarchy(root))
            roots.append(root)
            piece = placement.get("piece", "").lower()
            if getattr(root, "sollum_type", "") == "sollumz_drawable_dictionary" and piece:
                for child in root.children:  # (only the model the object uses, of a dictionary of them)
                    child.hide_set(child.name.rsplit(".", 1)[0].lower() != piece)
        root[OBJECT_KEY] = placement["id"]  # (a new id if RedForge restarted)
        root[WORLD_KEY] = list(position)
        selected.append(root)

    reload_pictures({Path(asset["xml"]).parent for asset in result["assets"]})
    _quiet_until = time.monotonic() + 1.0
    context.view_layer.update()  # (the import's updates, now while they're ignored)
    for obj in context.view_layer.objects.selected:
        obj.select_set(False)
    for root in selected:
        root.select_set(True)
        context.view_layer.objects.active = root
    errors = result.get("errors") or []
    message = f"{len(selected)} object(s) from RedForge, {imported} model(s) imported"
    return message + (f" ({len(errors)} failed: {errors[0]})" if errors else "")


# ---- Blender -> RedForge


def send(context) -> str:
    """Send the edited models (through Sollumz, RedForge puts them in its project) and where objects were moved to."""
    port = prefs(context).port
    roots = redforge_roots()
    origin = list(context.scene.get(ORIGIN_KEY, (0, 0, 0)))
    sent = []

    for root in roots:
        take_part_moves(root)
    staging = Path(tempfile.gettempdir()) / "RedForge" / "Blender" / "send"
    settings = sollumz("sollumz_preferences").get_export_settings()
    for file in sorted(_changed | {root[FILE_KEY] for root in roots if model_edited(root)}):
        users = [obj for obj in roots if obj[FILE_KEY] == file]
        if not users:
            _changed.discard(file)
            continue
        root = next((obj for obj in users if model_edited(obj)), users[0])  # (the one with the edited parts)
        exporter = EXPORTERS.get(file.rsplit(".", 1)[-1].lower())
        if exporter is None:
            raise RuntimeError(f"{file}: sending this kind of model isn't supported yet")
        folder = staging / file
        shutil.rmtree(folder, ignore_errors=True)
        folder.mkdir(parents=True)
        xml = folder / f"{file}.xml"
        with _Setting(settings, "apply_transforms", False):  # (the geometry goes back relative to the object)
            if not getattr(sollumz(exporter[0]), exporter[1])(root, str(xml)):
                raise RuntimeError(f"Sollumz couldn't export {file}")
        link.request(port, "import", xml=str(xml), file=file)
        _changed.discard(file)
        for user in users:
            mark_sent(user.children_recursive)
        sent.append(file)

    moves = 0
    for root in roots:
        if (OBJECT_KEY not in root) or not differs(root):
            continue
        location, rotation, scale = root.matrix_basis.decompose()
        position = [o + l for o, l in zip(origin, location)]
        link.request(port, "transform", id=root[OBJECT_KEY], position=position, rotation=list(rotation), scale=list(scale))
        root[SENT_KEY] = transform(root)
        root[WORLD_KEY] = position
        moves += 1

    sent += send_ytyps(context, {file.rsplit(".", 1)[0] for file in sent}) if sent else []
    if not sent and not moves:
        return "Nothing changed since the last send"
    return "Sent to RedForge: " + ", ".join(filter(None, (", ".join(sent), f"{moves} object(s) moved" if moves else "")))


def send_new(context, objects) -> str:
    """Models made here: named the way Sollumz's Export names them, sent with the ytyps that have archetypes for them.
    After that they're sent like the others when they change."""
    for root in {sollumz_root(obj) for obj in objects} - {None}:
        if FILE_KEY not in root:
            root[FILE_KEY] = f"{model_name(root.name)}.{SOLLUMZ_MODELS[root.sollum_type]}"
            mark_sent(hierarchy(root))
            _changed.add(root[FILE_KEY])
    return send(context)


def rdr_ytyps(scene):
    return [ytyp for ytyp in getattr(scene, "ytyps", []) if ytyp.game == "sollumz_rdr3"]


def send_ytyps(context, models=None, names=None) -> list[str]:
    """Sollumz's RDR ytyps (they're scene data, not objects), exported by Sollumz the way its Export ytyp.xml does:
    the ones with an archetype for one of the models (names without extension), or the ones named."""
    scene = context.scene
    exporter = sollumz("ytyp.ytypexport")
    apply_transforms = sollumz("sollumz_preferences").get_export_settings().apply_transforms
    staging = Path(tempfile.gettempdir()) / "RedForge" / "Blender" / "send"
    sent = []
    for index, ytyp in enumerate(getattr(scene, "ytyps", [])):
        if ytyp.game != "sollumz_rdr3":
            continue
        if (models is not None) and not any(model_name(a.asset.name if a.asset else a.asset_name) in models for a in ytyp.archetypes):
            continue
        if (names is not None) and (ytyp.name not in names):
            continue
        with _Setting(scene, "ytyp_index", index):  # (Sollumz exports the one selected in its panel)
            ytyp_xml = exporter.selected_ytyp_to_xml(apply_transforms)
        folder = staging / f"{ytyp_xml.name}.ytyp"
        shutil.rmtree(folder, ignore_errors=True)
        folder.mkdir(parents=True)
        xml = folder / f"{ytyp_xml.name}.ytyp.rsc.xml"
        ytyp_xml.write_xml(str(xml))
        link.request(prefs(context).port, "import", xml=str(xml), file=f"{ytyp_xml.name}.ytyp")
        sent.append(f"{ytyp_xml.name}.ytyp")
    return sent


def pending(roots) -> tuple[int, int]:
    """How many models were edited, and objects moved."""
    edited = (_changed & {root[FILE_KEY] for root in roots}) | {root[FILE_KEY] for root in roots if model_edited(root)}
    return len(edited), sum(1 for root in roots if moved(root))


class REDFORGE_OT_edit_selection(bpy.types.Operator):
    """Bring in the objects selected in RedForge, around the working origin"""
    bl_idname = "redforge.edit_selection"
    bl_label = "Edit RedForge Selection"
    bl_options = {"REGISTER", "UNDO"}

    def execute(self, context):
        try:
            self.report({"INFO"}, edit_selection(context))
        except Exception as ex:
            self.report({"ERROR"}, str(ex))
            return {"CANCELLED"}
        return {"FINISHED"}


class REDFORGE_OT_send(bpy.types.Operator):
    """Send the edited models and the moved objects to RedForge: the map shows them straight away"""
    bl_idname = "redforge.send"
    bl_label = "Send Changes to RedForge"

    @classmethod
    def poll(cls, context):
        return bool(redforge_roots())

    def execute(self, context):
        try:
            self.report({"INFO"}, send(context))
        except Exception as ex:
            self.report({"ERROR"}, str(ex))
            return {"CANCELLED"}
        return {"FINISHED"}


class REDFORGE_OT_send_new(bpy.types.Operator):
    """Send the selected Sollumz models made in Blender to RedForge, exported the way Sollumz's Export does, with the RDR ytyps that have archetypes for them"""
    bl_idname = "redforge.send_new"
    bl_label = "Send New Model to RedForge"

    @classmethod
    def poll(cls, context):
        return any((root is not None) and (FILE_KEY not in root) for root in map(sollumz_root, context.selected_objects))

    def execute(self, context):
        try:
            self.report({"INFO"}, send_new(context, context.selected_objects))
        except Exception as ex:
            self.report({"ERROR"}, str(ex))
            return {"CANCELLED"}
        return {"FINISHED"}


class REDFORGE_OT_toggle_ytyp(bpy.types.Operator):
    """Send this ytyp with Send YTYPs, or not"""
    bl_idname = "redforge.toggle_ytyp"
    bl_label = "Send This YTYP"
    bl_options = {"INTERNAL"}
    name: StringProperty()

    def execute(self, context):
        chosen = set(context.scene.get(YTYPS_KEY, [])) ^ {self.name}
        context.scene[YTYPS_KEY] = sorted(chosen)
        return {"FINISHED"}


class REDFORGE_OT_send_ytyps(bpy.types.Operator):
    """Send the ytyps ticked in the panel to RedForge, after changing their archetypes (Send New Model and Send Changes send the ones with archetypes for their models by themselves)"""
    bl_idname = "redforge.send_ytyps"
    bl_label = "Send YTYPs to RedForge"

    @classmethod
    def poll(cls, context):
        chosen = set(context.scene.get(YTYPS_KEY, []))
        return any(ytyp.name in chosen for ytyp in rdr_ytyps(context.scene))

    def execute(self, context):
        try:
            sent = send_ytyps(context, names=set(context.scene.get(YTYPS_KEY, [])))
            self.report({"INFO"}, f"Sent {', '.join(sent)} to RedForge")
        except Exception as ex:
            self.report({"ERROR"}, str(ex))
            return {"CANCELLED"}
        return {"FINISHED"}


class REDFORGE_OT_clean_exports(bpy.types.Operator):
    """Delete the textures and models RedForge exported for Blender that this .blend doesn't use. Other .blend files that use them get them back with Edit in Blender on those objects"""
    bl_idname = "redforge.clean_exports"
    bl_label = "Clean Up Exported Textures"

    def invoke(self, context, event):
        folders, size = unused_exports()
        if not folders:
            self.report({"INFO"}, "Nothing to clean: this .blend uses all of RedForge's exports")
            return {"CANCELLED"}
        return context.window_manager.invoke_confirm(self, event, title="Clean Up Exported Textures", confirm_text="Delete",
                                                     message=f"Delete {len(folders)} exported model(s), {size / 1048576:.1f} MB? Other .blend files using them get their textures back with Edit in Blender.")

    def execute(self, context):
        try:
            self.report({"INFO"}, clean_exports())
        except Exception as ex:
            self.report({"ERROR"}, str(ex))
            return {"CANCELLED"}
        return {"FINISHED"}


class REDFORGE_PT_panel(bpy.types.Panel):
    bl_label = "RedForge"
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "RedForge"

    def draw(self, context):
        layout = self.layout
        if link.connected():
            layout.label(text="Connected to RedForge", icon="LINKED")
        else:
            layout.label(text="Waiting for RedForge...", icon="UNLINKED")
        layout.operator(REDFORGE_OT_edit_selection.bl_idname, icon="IMPORT")
        layout.operator(REDFORGE_OT_send.bl_idname, icon="EXPORT")
        layout.operator(REDFORGE_OT_send_new.bl_idname, text="Send New Model", icon="ADD")
        ytyps = rdr_ytyps(context.scene)
        if ytyps:
            box = layout.box()
            box.label(text="YTYPs to send", icon="PRESET")
            chosen = set(context.scene.get(YTYPS_KEY, []))
            for ytyp in ytyps:
                ticked = ytyp.name in chosen
                box.operator(REDFORGE_OT_toggle_ytyp.bl_idname, text=ytyp.name, icon="CHECKBOX_HLT" if ticked else "CHECKBOX_DEHLT", emboss=False).name = ytyp.name
            count = sum(1 for ytyp in ytyps if ytyp.name in chosen)
            box.operator(REDFORGE_OT_send_ytyps.bl_idname, text=f"Send {count} YTYP(s)" if count else "Send YTYPs", icon="EXPORT")
        layout.prop(prefs(context), "send_on_save")
        layout.operator(REDFORGE_OT_clean_exports.bl_idname, icon="TRASH")
        roots = redforge_roots()
        models, moves = pending(roots)
        if models or moves:
            layout.label(text=f"To send: {models} model(s), {moves} moved", icon="ERROR")
        if ORIGIN_KEY in context.scene:
            x, y, z = context.scene[ORIGIN_KEY]
            layout.label(text=f"Origin {x:.1f}, {y:.1f}, {z:.1f}", icon="ORIENTATION_GLOBAL")
        root = model_root(context.active_object)
        if root is not None:
            box = layout.box()
            box.label(text=root[FILE_KEY], icon="MESH_CUBE")
            if (root[FILE_KEY] in _changed) or model_edited(root):
                box.label(text="Model edited")
            if moved(root):
                box.label(text="Moved")


class RedForgePreferences(bpy.types.AddonPreferences):
    bl_idname = __package__

    port: IntProperty(name="Port", description="RedForge's bridge port (its Bridge.Port setting)", default=47810, min=1, max=65535,
                      update=lambda self, context: link.start(self.port))
    send_on_save: BoolProperty(name="Send When Saving", description="Send the changes to RedForge every time the file is saved", default=False)

    def draw(self, context):
        self.layout.prop(self, "port")
        self.layout.prop(self, "send_on_save")


# ---- handlers


def _tick():
    """On the main thread: RedForge's events, and the connection shown in the panel."""
    global _was_connected
    if link.listening() is None:
        try:
            link.start(prefs().port)  # (the preferences only exist once the add-on is enabled, after register)
        except KeyError:
            return 1.0
    while not link.events.empty():
        event = link.events.get()
        if event.get("event") == "edit":
            try:
                print("RedForge Bridge:", edit_selection(bpy.context))
            except Exception as ex:
                print("RedForge Bridge: edit failed:", ex)
    if link.connected() != _was_connected:
        _was_connected = link.connected()
        redraw()
    return 0.25


@persistent
def _on_depsgraph(scene, depsgraph):
    if time.monotonic() < _quiet_until:
        return
    # (the mesh data itself: an object's geometry also updates when it's moved with an armature, which isn't an edit)
    meshes = {u.id.original for u in depsgraph.updates if u.is_updated_geometry and isinstance(u.id, bpy.types.Mesh)}
    materials = {u.id.original for u in depsgraph.updates if u.is_updated_shading and isinstance(u.id, bpy.types.Material)}
    if not meshes and not materials:
        return
    for root in redforge_roots():
        if any((obj.data in meshes) or any(m in materials for m in obj.data.materials) for obj in hierarchy(root) if obj.type == "MESH"):
            _changed.add(root[FILE_KEY])


@persistent
def _on_save_pre(*_):
    bpy.context.scene[CHANGED_KEY] = sorted(_changed)


@persistent
def _on_save_post(*_):
    if prefs().send_on_save and any(pending(redforge_roots())):
        try:
            print("RedForge Bridge:", send(bpy.context))
        except Exception as ex:
            print("RedForge Bridge: send failed:", ex)


@persistent
def _on_load(*_):
    _changed.clear()
    _changed.update(bpy.context.scene.get(CHANGED_KEY, []))


HANDLERS = (
    (bpy.app.handlers.depsgraph_update_post, _on_depsgraph),
    (bpy.app.handlers.save_pre, _on_save_pre),
    (bpy.app.handlers.save_post, _on_save_post),
    (bpy.app.handlers.load_post, _on_load),
)
CLASSES = (RedForgePreferences, REDFORGE_OT_edit_selection, REDFORGE_OT_send, REDFORGE_OT_send_new, REDFORGE_OT_toggle_ytyp, REDFORGE_OT_send_ytyps, REDFORGE_OT_clean_exports, REDFORGE_PT_panel)


def register():
    for cls in CLASSES:
        bpy.utils.register_class(cls)
    for handlers, handler in HANDLERS:
        handlers.append(handler)
    bpy.app.timers.register(_tick, persistent=True)


def unregister():
    link.stop()
    if bpy.app.timers.is_registered(_tick):
        bpy.app.timers.unregister(_tick)
    for handlers, handler in HANDLERS:
        handlers.remove(handler)
    for cls in reversed(CLASSES):
        bpy.utils.unregister_class(cls)
