# blender -b --factory-startup --python-exit-code 1 --python scripts/blender_check.py -- <file.glb> [budget]
# .glb 를 import_scene.gltf 로 가져와서 메시 1개, 재질 1개, 색 속성 없음, 삼각형 예산, 발바닥 원점을 검사한다.
import sys

import bpy

args = sys.argv[sys.argv.index("--") + 1 :]
path = args[0]
budget = int(args[1]) if len(args) > 1 else 2000

bpy.ops.wm.read_factory_settings(use_empty=True)
result = bpy.ops.import_scene.gltf(filepath=path)
assert result == {"FINISHED"}, f"import failed: {result}"

# 뼈대가 든 파일을 가져오면 Blender 가 뼈 표시용 물체(Icosphere)를 스스로 더한다. 파일에 든 메시가 아니다.
meshes = [o for o in bpy.context.scene.objects if o.type == "MESH" and not o.name.startswith("Icosphere")]
assert len(meshes) == 1, f"mesh objects: {len(meshes)}"
obj = meshes[0]
mesh = obj.data
assert len(mesh.materials) == 1, f"materials: {len(mesh.materials)}"
mesh.calc_loop_triangles()
tris = len(mesh.loop_triangles)
assert 0 < tris <= budget, f"triangles {tris} > {budget}"
assert len(mesh.color_attributes) == 0, "unexpected color attribute"
zs = [(obj.matrix_world @ v.co).z for v in mesh.vertices]
assert abs(min(zs)) < 0.02, f"min z {min(zs):.3f}: origin is not at the sole"
print(f"blender check ok: 1 mesh, 1 material, {tris} triangles, height {max(zs) - min(zs):.3f} m")
