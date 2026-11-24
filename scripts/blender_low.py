# 앱이 부르는 Blender 스크립트. src/lowpoly.rs 가 include_str! 로 넣고 작업 폴더에 써서 실행한다.
# 사용: blender -b --factory-startup --python-exit-code 1 --python blender_low.py -- <입력 ply> <출력 ply> <삼각형 수>
# 촘촘한 메시를 닫힌 한 겹으로 다시 만들고(voxel remesh) 삼각형 수까지 줄인다. 좌표는 바꾸지 않는다.
# 열린 얇은 판이 많은 메시는 remesh 가 잔 조각 수천 개로 부순다. 형태가 뜯기고, 심하면 삼각형 수까지 줄지 않는다.
# 그때만 열린 덩어리에 두께를 주고 처음부터 다시 한다. 첫 remesh 가 멀쩡한 메시의 결과는 그대로다.
import sys

import bmesh
import bpy

src, dst, target = sys.argv[sys.argv.index("--") + 1 :][:3]
target = int(target)
# voxel 크기: 가장 긴 변을 이 수로 나눈 값. 이보다 얇은 조각은 사라진다.
VOXELS = 320
# 열린 덩어리에 주는 두께(voxel 수). 1.0~2.5 에서 IoU 차이가 0.01 안이었다(changes/019).
THICKNESS = 1.5
# 첫 remesh 가 낸 덩어리 수가 이보다 많으면 부서진 것으로 본다.
# 멀쩡한 메시는 1~10 개, 부서진 메시는 1081~3426 개였다(changes/019).
MAX_ISLANDS = 100

bpy.ops.wm.read_factory_settings(use_empty=True)
# 축을 기본값(Y 앞, Z 위)으로 읽고 같은 값으로 쓴다. 좌표가 그대로 돌아온다.
bpy.ops.wm.ply_import(filepath=src)
obj = next(o for o in bpy.context.scene.objects if o.type == "MESH")
bpy.context.view_layer.objects.active = obj
obj.select_set(True)
original = obj.data.copy()
voxel = max(obj.dimensions) / VOXELS


def triangles():
    obj.data.calc_loop_triangles()
    return len(obj.data.loop_triangles)


def islands(bm):
    """변으로 이어진 정점 덩어리들."""
    seen, out = set(), []
    for start in bm.verts:
        if start.index in seen:
            continue
        seen.add(start.index)
        stack, island = [start], []
        while stack:
            v = stack.pop()
            island.append(v)
            for e in v.link_edges:
                o = e.other_vert(v)
                if o.index not in seen:
                    seen.add(o.index)
                    stack.append(o)
        out.append(island)
    return out


def reduce(max_islands=None):
    """remesh 한 뒤 삼각형 수까지 줄이고 (삼각형 수, remesh 가 낸 덩어리 수)를 낸다.
    max_islands 를 주면 덩어리가 그보다 많을 때 줄이지 않고 돌아온다."""
    # 겹친 면, 안쪽 면, 작은 구멍을 없애고 바깥 표면 한 겹만 남긴다.
    remesh = obj.modifiers.new("remesh", "REMESH")
    remesh.mode = "VOXEL"
    remesh.voxel_size = voxel
    remesh.adaptivity = 0.0
    bpy.ops.object.modifier_apply(modifier=remesh.name)
    bm = bmesh.new()
    bm.from_mesh(obj.data)
    count = len(islands(bm))
    bm.free()
    if max_islands is not None and count > max_islands:
        return triangles(), count

    for _ in range(4):
        n = triangles()
        if n <= target:
            break
        decimate = obj.modifiers.new("decimate", "DECIMATE")
        decimate.decimate_type = "COLLAPSE"
        decimate.ratio = target * 0.99 / n
        decimate.use_collapse_triangulate = True
        bpy.ops.object.modifier_apply(modifier=decimate.name)
    return triangles(), count


def thicken_open():
    """경계 변이 있는 덩어리(열린 판)의 정점에만 두께를 준다. 닫힌 덩어리는 부풀리지 않는다."""
    bm = bmesh.new()
    bm.from_mesh(obj.data)
    open_verts = [
        v.index
        for island in islands(bm)
        if any(len(e.link_faces) == 1 for v in island for e in v.link_edges)
        for v in island
    ]
    bm.free()
    group = obj.vertex_groups.new(name="open")
    group.add(open_verts, 1.0, "REPLACE")
    solid = obj.modifiers.new("solidify", "SOLIDIFY")
    solid.thickness = THICKNESS * voxel
    solid.offset = 0.0
    solid.vertex_group = group.name
    solid.thickness_vertex_group = 0.0
    bpy.ops.object.modifier_apply(modifier=solid.name)
    return len(open_verts)


n, count = reduce(MAX_ISLANDS)
if n > target or count > MAX_ISLANDS:
    obj.data = original
    opened = thicken_open()
    n, again = reduce()
    print(f"low poly retry: remesh made {count} islands, thickened {opened} open vertices, then {again} islands")
if n == 0 or n > target:
    sys.exit(f"decimate ended at {n} triangles, target {target}")
bpy.ops.wm.ply_export(
    filepath=dst,
    export_selected_objects=True,
    export_uv=False,
    export_normals=False,
    export_colors="NONE",
    export_attributes=False,
    export_triangulated_mesh=True,
    ascii_format=False,
)
print(f"low poly ok: {n} triangles")
