# 기준 마네킹(Universal Animation Library 의 UAL1_Standard.glb, CC0)의 뼈대를 schema/mannequin_skeleton.json 으로 뽑는다.
# 사용: blender -b --factory-startup --python scripts/make_skeleton.py -- <UAL1_Standard.glb> schema/mannequin_skeleton.json
# 뼈마다 이름, 부모, Blender rest 행렬(armature 공간, bone_heuristic=BLENDER), 길이, glTF 의 local rotation 을 담는다.
import json
import struct
import sys

import bpy

src, dst = sys.argv[sys.argv.index("--") + 1 :][:2]
raw = open(src, "rb").read()
doc = json.loads(raw[20 : 20 + struct.unpack("<I", raw[12:16])[0]])
gltf_rot = {n.get("name"): n.get("rotation", [0, 0, 0, 1]) for n in doc["nodes"]}

bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=src, bone_heuristic="BLENDER")
arm = next(o for o in bpy.context.scene.objects if o.type == "ARMATURE")
bones = []


def walk(b):
    bones.append(
        {
            "name": b.name,
            "parent": b.parent.name if b.parent else None,
            "matrix": [[round(v, 6) for v in row] for row in b.matrix_local],
            "length": round(b.length, 6),
            "rotation": [round(v, 6) for v in gltf_rot[b.name]],
        }
    )
    for c in b.children:
        walk(c)


for b in arm.data.bones:
    if b.parent is None:
        walk(b)
with open(dst, "w") as f:
    json.dump({"source": "Universal Animation Library, UAL1_Standard.glb (Quaternius, CC0)", "bones": bones}, f, indent=1)
    f.write("\n")
print(f"skeleton ok: {len(bones)} bones")
