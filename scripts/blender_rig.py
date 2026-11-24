# 앱이 부르는 Blender 스크립트. src/rig.rs 가 include_str! 로 넣고 작업 폴더에 써서 실행한다.
# 사용: blender -b --factory-startup --python-exit-code 1 --python blender_rig.py -- <입력 ply> <관절점 txt> <뼈대 json> <출력 glb>
# 사람 메시에 기준 마네킹의 뼈대(이름·구조·rest 방향이 같다)를 넣는다. 뼈의 자리는 이 인물의 관절점에 맞춘다.
# 입력 ply 는 앱의 좌표(Y 위, +Z 앞, +X 인물의 왼쪽)다. 관절점은 "이름 x y" 줄이다. 출력은 T포즈의 skinned glb 다.
import json
import sys

import bpy
from mathutils import Matrix, Vector

src, joints_path, skeleton_path, dst = sys.argv[sys.argv.index("--") + 1 :][:4]
GRAY = 0.6

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
view = bpy.context.view_layer


def only(*objs):
    for o in scene.objects:
        o.select_set(o in objs)
    view.objects.active = objs[-1]


# 1. 메시: 앱의 Y-up 을 Blender 의 Z-up 으로 돌린다. 인물은 -Y 를 본다(마네킹과 같다).
bpy.ops.wm.ply_import(filepath=src)
body = next(o for o in scene.objects if o.type == "MESH")
body.name = body.data.name = "mytripo_public"
body.data.transform(Matrix(((1, 0, 0, 0), (0, 0, -1, 0), (0, 1, 0, 0), (0, 0, 0, 1))))
body.data.update()
view.update()


def depth(x, z):
    """앞과 뒤에서 쏜 광선이 처음 닿는 곳의 가운데. 닿지 않으면 None."""
    dg = bpy.context.evaluated_depsgraph_get()
    a = scene.ray_cast(dg, Vector((x, -5, z)), Vector((0, 1, 0)))
    b = scene.ray_cast(dg, Vector((x, 5, z)), Vector((0, -1, 0)))
    return (a[1].y + b[1].y) / 2 if a[0] and b[0] else None


J = {}
for line in open(joints_path):
    name, x, y = line.split()
    x, z = float(x), float(y)
    J[name] = Vector((x, depth(x, z) or 0.0, z))
need = ["head_top", "neck", "pelvis"] + [p + s for s in ("_l", "_r") for p in ("shoulder", "elbow", "wrist", "hand", "hip", "knee", "ankle", "foot")]
missing = [n for n in need if n not in J]
if missing:
    sys.exit(f"missing joints: {missing}")

# 2. 마네킹 뼈대
bones = json.load(open(skeleton_path))["bones"]
order = [b["name"] for b in bones]
parent = {b["name"]: b["parent"] for b in bones}
T = {b["name"]: Matrix(b["matrix"]) for b in bones}
length = {b["name"]: b["length"] for b in bones}
Th = {n: T[n].to_translation() for n in order}

# 3. 뼈마다 이 인물에서의 자리. 몸통은 골반~목 사이를 마네킹의 비율로 나눈다.
anchors = sorted([(0.0, 0.0), (Th["pelvis"].z, J["pelvis"].z), (Th["Head"].z, J["neck"].z)])


def fz(z):
    for (a0, b0), (a1, b1) in zip(anchors, anchors[1:]):
        if z <= a1:
            return b0 + (b1 - b0) * (z - a0) / (a1 - a0)
    return anchors[-1][1]


def center(n):
    z = fz(Th[n].z)
    return Vector((0, depth(0, z) or 0.0, z))


P = {"root": Vector((0, 0, 0))}
for n in ("pelvis", "spine_01", "spine_02", "spine_03", "neck_01", "Head"):
    P[n] = center(n)
for s in "lr":
    P["upperarm_" + s], P["lowerarm_" + s], P["hand_" + s] = J["shoulder_" + s], J["elbow_" + s], J["wrist_" + s]
    P["thigh_" + s], P["calf_" + s], P["foot_" + s] = J["hip_" + s], J["knee_" + s], J["ankle_" + s]
    c = Th["clavicle_" + s]
    P["clavicle_" + s] = Vector((c.x * J["shoulder_" + s].x / Th["upperarm_" + s].x, P["spine_03"].y, fz(c.z)))
# 뼈가 가리키는 다음 자리
aim = {"spine_01": "spine_02", "spine_02": "spine_03", "spine_03": "neck_01", "neck_01": "Head"}
aim_point = {}
for s in "lr":
    aim.update({"clavicle_" + s: "upperarm_" + s, "upperarm_" + s: "lowerarm_" + s, "lowerarm_" + s: "hand_" + s})
    aim.update({"thigh_" + s: "calf_" + s, "calf_" + s: "foot_" + s})
    aim_point["hand_" + s] = J["hand_" + s]
# 손가락과 발끝은 부모를 따라간다. 부모로부터의 거리는 팔 길이의 비율로 줄인다.
arm_len = lambda q, a, b, c: (q[b] - q[a]).length + (q[c] - q[b]).length
scale = arm_len(P, "upperarm_l", "lowerarm_l", "hand_l") / arm_len(Th, "upperarm_l", "lowerarm_l", "hand_l")


def rot4(m):
    return m.to_3x3().to_4x4()


# 4. 이 인물의 A포즈에 맞춘 뼈대: 마네킹의 rest 에서 가장 적게 돌려 이 인물의 팔다리 방향을 가리킨다.
FIT, LEN = {}, {}
for n in order:
    p = parent[n]
    LEN[n] = length[n] * scale
    if n in P:
        if n in aim:
            t_dir, o_dir = Th[aim[n]] - Th[n], P[aim[n]] - P[n]
            LEN[n] = max(o_dir.length, 0.01)
        elif n in aim_point:
            t_dir, o_dir = T[n].to_3x3() @ Vector((0, 1, 0)), aim_point[n] - P[n]
        else:
            t_dir = o_dir = None
        turn = t_dir.rotation_difference(o_dir).to_matrix().to_4x4() if o_dir is not None and o_dir.length > 1e-6 else Matrix.Identity(4)
        FIT[n] = Matrix.Translation(P[n]) @ turn @ rot4(T[n])
    else:
        rel = T[p].inverted() @ T[n]
        rel.translation *= scale
        FIT[n] = FIT[p] @ rel

data = bpy.data.armatures.new("Rig")
arm = bpy.data.objects.new("Rig", data)
scene.collection.objects.link(arm)
only(arm)
bpy.ops.object.mode_set(mode="EDIT")
for n in order:
    eb = data.edit_bones.new(n)
    eb.head, eb.tail = (0, 0, 0), (0, LEN[n], 0)
    eb.matrix = FIT[n]
    eb.length = LEN[n]
    if parent[n]:
        eb.parent = data.edit_bones[parent[n]]
bpy.ops.object.mode_set(mode="OBJECT")

# 5. 메시를 뼈에 붙인다(자동 가중치). 붙지 않은 정점은 가장 가까운 뼈에 붙인다.
only(body, arm)
bpy.ops.object.parent_set(type="ARMATURE_AUTO")
loose = [v for v in body.data.vertices if not v.groups]
if loose:
    segs = [(b.name, b.head_local.copy(), b.tail_local.copy()) for b in data.bones]

    def near(p):
        def dist(seg):
            _, a, b = seg
            t = max(0.0, min(1.0, (p - a).dot(b - a) / max((b - a).length_squared, 1e-12)))
            return (p - (a + (b - a) * t)).length

        return min(segs, key=dist)[0]

    for v in loose:
        name = near(v.co)
        (body.vertex_groups.get(name) or body.vertex_groups.new(name=name)).add([v.index], 1.0, "REPLACE")
print(f"bound: {len(body.data.vertices)} vertices, {len(loose)} by nearest bone")

# 6. 뼈의 방향을 마네킹의 rest 와 같게 돌린다(관절 자리는 그대로). 메시는 마네킹의 T포즈가 된다.
rest = {b.name: b.matrix_local.copy() for b in data.bones}
POSED = {}
for n in order:
    p = parent[n]
    head = rest[n].to_translation() if p is None else (POSED[p] @ (rest[p].inverted() @ rest[n])).to_translation()
    POSED[n] = Matrix.Translation(head) @ rot4(T[n])
only(arm)
bpy.ops.object.mode_set(mode="POSE")
for n in order:
    arm.pose.bones[n].matrix = POSED[n]
    view.update()
bpy.ops.object.mode_set(mode="OBJECT")
only(body)
bpy.ops.object.modifier_apply(modifier=next(m for m in body.modifiers if m.type == "ARMATURE").name)
only(arm)
bpy.ops.object.mode_set(mode="POSE")
bpy.ops.pose.armature_apply(selected=False)
bpy.ops.object.mode_set(mode="OBJECT")
body.modifiers.new("Armature", "ARMATURE").object = arm
off = max((data.bones[n].matrix_local.to_3x3() - T[n].to_3x3()).row[i].length for n in order for i in range(3))
if off > 1e-3:
    sys.exit(f"rest orientation differs from the mannequin by {off}")

# 7. 발바닥을 바닥에 놓고 회색 재질 하나로 낸다.
low = min((body.matrix_world @ v.co).z for v in body.data.vertices)
only(arm)
bpy.ops.object.mode_set(mode="EDIT")
for eb in data.edit_bones:
    if eb.parent is None:
        continue
    eb.translate(Vector((0, 0, -low)))
bpy.ops.object.mode_set(mode="OBJECT")
body.data.transform(Matrix.Translation((0, 0, -low)))
mat = bpy.data.materials.new("mytripo_public")
mat.diffuse_color = (GRAY, GRAY, GRAY, 1)
mat.use_nodes = True
bsdf = next(n for n in mat.node_tree.nodes if n.type == "BSDF_PRINCIPLED")
bsdf.inputs["Base Color"].default_value = (GRAY, GRAY, GRAY, 1)
bsdf.inputs["Metallic"].default_value = 0.0
bsdf.inputs["Roughness"].default_value = 0.9
body.data.materials.clear()
body.data.materials.append(mat)
only(body, arm)
bpy.ops.export_scene.gltf(
    filepath=dst,
    export_format="GLB",
    use_selection=True,
    export_animations=False,
    export_yup=True,
    export_skins=True,
    export_normals=True,
    export_texcoords=False,
    export_vertex_color="NONE",
    export_morph=False,
    export_def_bones=False,
)
body.data.calc_loop_triangles()
print(f"rig ok: {len(data.bones)} bones, {len(body.data.loop_triangles)} triangles")
