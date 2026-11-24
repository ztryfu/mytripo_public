# test/fixtures 의 사진 3장을 만든다. 사람 모양 블록 인형을 직교 카메라로 렌더한다.
# 실행: blender -b --factory-startup --python test/fixtures/make_fixtures.py -- test/fixtures
# photo_a = 옆(흰 배경), photo_b = 앞(초록 배경), photo_c = 뒤(초록 배경). 파일 이름은 시점을 드러내지 않는다.
# sheet = 앞·옆·뒤가 한 장에 든 턴어라운드 시트(초록 배경).
import math
import os
import sys

import bpy
from mathutils import Vector

out_dir = sys.argv[sys.argv.index("--") + 1] if "--" in sys.argv else "."


def lin(r, g, b):
    return tuple((c / 255.0) ** 2.2 for c in (r, g, b)) + (1.0,)


SKIN = lin(222, 172, 136)
SHIRT = lin(200, 52, 48)
PANTS = lin(44, 62, 122)
SHOES = lin(42, 32, 26)
HAIR = lin(72, 46, 30)
DARK = lin(20, 16, 14)

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene


def finish(obj, color):
    obj.color = color
    return obj


def cylinder(a, b, r1, r2, color):
    a, b = Vector(a), Vector(b)
    d = b - a
    bpy.ops.mesh.primitive_cone_add(vertices=32, radius1=r1, radius2=r2, depth=d.length, location=(a + b) / 2)
    o = bpy.context.object
    o.rotation_mode = "QUATERNION"
    o.rotation_quaternion = Vector((0, 0, 1)).rotation_difference(d.normalized())
    return finish(o, color)


def sphere(c, r, color, scale=(1, 1, 1)):
    bpy.ops.mesh.primitive_uv_sphere_add(segments=32, ring_count=16, radius=r, location=c)
    o = bpy.context.object
    o.scale = scale
    return finish(o, color)


def box(c, size, color):
    bpy.ops.mesh.primitive_cube_add(size=1, location=c)
    o = bpy.context.object
    o.scale = size
    return finish(o, color)


def make_figure(name):
    """인형 한 벌을 만들고 원점의 empty 에 묶는다."""
    before = set(scene.objects)
    # 다리와 신발
    for s in (-1, 1):
        cylinder((0.10 * s, 0, 0.06), (0.10 * s, 0, 0.88), 0.05, 0.078, PANTS)
        box((0.10 * s, -0.04, 0.035), (0.10, 0.24, 0.07), SHOES)
    # 골반, 몸통, 목, 머리
    o = cylinder((0, 0, 0.80), (0, 0, 0.96), 0.165, 0.16, PANTS)
    o.scale.y *= 0.7
    o = cylinder((0, 0, 0.94), (0, 0, 1.42), 0.15, 0.185, SHIRT)
    o.scale.y *= 0.65
    cylinder((0, 0, 1.38), (0, 0, 1.50), 0.05, 0.05, SKIN)
    sphere((0, 0, 1.585), 0.11, SKIN, (1, 1, 1.15))
    sphere((0, 0.025, 1.61), 0.115, HAIR, (1.02, 1, 1.1))
    for s in (-1, 1):
        sphere((0.04 * s, -0.103, 1.59), 0.013, DARK)
    box((0, -0.104, 1.53), (0.05, 0.01, 0.012), DARK)
    # 팔 (A 포즈, 수직에서 25도)
    ang = math.radians(25)
    for s in (-1, 1):
        sh = Vector((0.20 * s, 0, 1.38))
        d = Vector((math.sin(ang) * s, 0, -math.cos(ang)))
        el = sh + d * 0.30
        wr = el + d * 0.27
        cylinder(sh, el, 0.055, 0.045, SHIRT)
        cylinder(el, wr, 0.04, 0.034, SKIN)
        sphere(wr + d * 0.06, 0.05, SKIN, (1, 0.6, 1.3))
        sphere(sh, 0.06, SHIRT)


    root = bpy.data.objects.new(name, None)
    scene.collection.objects.link(root)
    for o in set(scene.objects) - before:
        if o is not root:
            o.parent = root
    return root


make_figure("single")

# 렌더 설정
scene.render.engine = "BLENDER_WORKBENCH"
scene.display.shading.light = "STUDIO"
scene.display.shading.color_type = "OBJECT"
scene.display.shading.show_object_outline = False
scene.display.shading.show_cavity = False
scene.display.shading.show_shadows = False
scene.view_settings.view_transform = "Standard"
scene.render.resolution_x = 768
scene.render.resolution_y = 1024
scene.render.resolution_percentage = 100
scene.render.film_transparent = False
scene.render.image_settings.file_format = "PNG"
scene.render.image_settings.color_mode = "RGB"
world = bpy.data.worlds.new("bg")
scene.world = world

cam_data = bpy.data.cameras.new("cam")
cam_data.type = "ORTHO"
cam_data.ortho_scale = 2.0
cam = bpy.data.objects.new("cam", cam_data)
scene.collection.objects.link(cam)
scene.camera = cam

GREEN = (0.0, 0.44, 0.05)
WHITE = (1.0, 1.0, 1.0)
shots = [
    ("photo_a.png", (5, 0, 0.9), (90, 0, 90), WHITE),
    ("photo_b.png", (0, -5, 0.9), (90, 0, 0), GREEN),
    ("photo_c.png", (0, 5, 0.9), (90, 0, 180), GREEN),
]
for name, loc, rot, bg in shots:
    cam.location = loc
    cam.rotation_euler = tuple(math.radians(v) for v in rot)
    world.color = bg
    scene.display.shading.background_type = "WORLD"
    scene.render.filepath = os.path.join(os.path.abspath(out_dir), name)
    bpy.ops.render.render(write_still=True)
    print("wrote", scene.render.filepath)

# 턴어라운드 시트(sheet.png): 한 장에 왼쪽부터 앞·옆(왼쪽을 봄)·뒤. 인형 세 벌을 Z 축으로 0·−90·180도 돌린다.
for o in list(scene.objects):
    if o.type != "CAMERA":
        bpy.data.objects.remove(o)
for k, (dx, rz) in enumerate([(-1.3, 0), (0.0, -90), (1.3, 180)]):
    root = make_figure(f"view{k}")
    root.location = (dx, 0, 0)
    root.rotation_euler = (0, 0, math.radians(rz))
scene.render.resolution_x = 1536
scene.render.resolution_y = 768
cam_data.ortho_scale = 4.0
cam.location = (0, -5, 0.9)
cam.rotation_euler = (math.radians(90), 0, 0)
world.color = GREEN
scene.render.filepath = os.path.join(os.path.abspath(out_dir), "sheet.png")
bpy.ops.render.render(write_still=True)
print("wrote", scene.render.filepath)
