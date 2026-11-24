# 사람형 rig(.rig.glb)에 자세 JSON 의 동작을 넣고, 검사하고, 비교 시트를 그린다. 앱은 부르지 않는다.
# 사용(레포에서):
#   blender -b --factory-startup --python-exit-code 1 --python scripts/blender_anim.py -- make  <rig.glb> <pose.json> <out.anim.glb>
#   blender -b --factory-startup --python-exit-code 1 --python scripts/blender_anim.py -- check <anim.glb> <pose.json>
#   blender -b --factory-startup --python-exit-code 1 --python scripts/blender_anim.py -- sheet <anim.glb> <pose.json> <out.png> [참고 이미지]
#
# 좌표(Blender 안): 인물은 -Y 를 보고, +X 는 인물의 왼쪽, +Z 는 위다. 길이는 미터, 각도는 도.
# 뼈 각도 [x, y, z] 는 rest(T포즈)의 축으로 적은 부모 기준 회전이다. Y 를 먼저, X 를, Z 를 나중에 돌린다.
#   척추·목·머리: x+ 는 앞으로 숙임, y+ 는 인물의 왼쪽으로 기울임, z+ 는 인물의 왼쪽으로 돌림.
#   왼팔(upperarm_l): y+ 는 내림, x- 는 앞으로, z- 는 앞으로 모음. 오른팔은 y 와 z 의 부호가 반대다.
#   아래팔(lowerarm_l): z- 는 앞으로 굽힘. 오른쪽은 z+.
# 발은 IK 로 바닥에 고정한다. feet.l = [dx, dy, dz, rz] 는 rest 발목 자리에서 옮긴 거리와 Z 축 회전이다.
# pelvis_loc = [dx, dy, dz] 는 골반을 옮긴 거리다. curl = {"l": 도, "r": 도} 는 엄지를 뺀 손가락을 굽힌다.
#
# 자세 JSON:
#   {"fps": 30,
#    "poses": {"이름": {"bones": {...}, "pelvis_loc": [...], "feet": {"l": [...], "r": [...]}, "curl": {...}}},
#    "clips": [
#      {"name": "enter", "from": "neutral", "to": "stance", "frames": 14, "ease": "out_back",
#       "steps": [{"foot": "l", "start": 0.0, "end": 0.45, "lift": 0.06}]},
#      {"name": "idle", "pose": "stance", "frames": 60, "loop": true,
#       "waves": [{"key": "spine_02", "amp": [1.5, 0, 0], "phase": 0}]}],
#    "views": {"ref": [-3.4, -4.9, 1.0]}, "compare": ["front", "side", "three4"],
#    "check": {"foot_slide": 0.01, "floor": 0.01, "seam": 0.001}}
# 전환 클립(from → to)은 뼈 회전을 slerp 로 섞는다. 유지 클립(pose)의 waves 는 그 자세에 sin 흔들림을 더하고,
# 첫 프레임에서 0 이다. "pelvis_loc" 와 "foot_l" · "foot_r" 도 key 로 쓸 수 있다.
# 이름이 있는 자세는 한 번만 풀어 둔다. 클립의 첫·끝 프레임이 그 자세면 같은 값을 쓰므로 클립 사이 이음새가 맞는다.
import json
import math
import os
import sys

import bpy
from mathutils import Matrix, Quaternion, Vector

D = math.radians
LEG = ["thigh_l", "calf_l", "foot_l", "thigh_r", "calf_r", "foot_r"]
DEFAULT_VIEWS = {"front": [0, -6, 0.95], "side": [6, 0, 0.95], "three4": [4.2, -4.2, 1.4]}
DEFAULT_CHECK = {"foot_slide": 0.01, "floor": 0.01, "seam": 0.001}


def load(path, fps):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.context.scene.render.fps = fps
    bpy.ops.import_scene.gltf(filepath=path)
    # 뼈대가 든 파일을 가져오면 Blender 가 뼈 표시용 Icosphere 를 더한다. 파일의 메시가 아니다.
    for o in list(bpy.data.objects):
        if o.type == "MESH" and o.name.startswith("Icosphere"):
            bpy.data.objects.remove(o)
    arm = next(o for o in bpy.data.objects if o.type == "ARMATURE")
    body = next(o for o in bpy.data.objects if o.type == "MESH")
    return arm, body


def q_of(e):
    x, y, z = e
    return Quaternion((0, 0, 1), D(z)) @ Quaternion((1, 0, 0), D(x)) @ Quaternion((0, 1, 0), D(y))


def lerp(a, b, t):
    return [x + (y - x) * t for x, y in zip(a, b)]


def qmix(qa, qb, t):
    """qa 에서 qb 로 가는 최단 회전의 t 배. t 가 1 을 넘으면 지나쳐 간다(out_back)."""
    if qa.dot(qb) < 0:
        qb = -qb
    axis, ang = (qa.inverted() @ qb).to_axis_angle()
    return qa @ Quaternion(axis, ang * t)


def curl_bones(side, deg):
    sg = 1 if side == "l" else -1
    return {f"{f}_0{i}_{side}": [0, sg * deg * k, 0] for f in ("index", "middle", "ring", "pinky") for i, k in ((1, 0.8), (2, 1.0), (3, 0.8))}


def flat(p):
    """자세 JSON 하나를 {뼈: 오일러, pelvis_loc, foot_l, foot_r} 로 편다."""
    out = {k: list(v) for k, v in p.get("bones", {}).items()}
    for s, deg in p.get("curl", {}).items():
        for k, v in curl_bones(s, deg).items():
            out.setdefault(k, v)
    out["pelvis_loc"] = list(p.get("pelvis_loc", [0, 0, 0]))
    for s in "lr":
        out["foot_" + s] = list(p.get("feet", {}).get(s, [0, 0, 0, 0]))
    return out


def ease(name, t):
    if name == "linear":
        return t
    if name == "in_out":
        return t * t * (3 - 2 * t)
    if name == "out":
        return 1 - (1 - t) ** 2
    if name == "out_back":
        s, u = 1.4, t - 1
        return 1 + (s + 1) * u**3 + s * u**2
    sys.exit(f"unknown ease: {name}")


class Rig:
    def __init__(self, arm):
        self.arm = arm
        for pb in arm.pose.bones:
            pb.rotation_mode = "QUATERNION"
        self.rest = {b.name: b.matrix_local.copy() for b in arm.data.bones}
        self.ankle = {s: (arm.matrix_world @ self.rest["calf_" + s] @ Vector((0, arm.data.bones["calf_" + s].length, 0))) for s in "lr"}
        self.tgt, self.pole = {}, {}
        for s in "lr":
            self.tgt[s] = self.empty("ik_" + s)
            self.pole[s] = self.empty("pole_" + s)
            c = arm.pose.bones["calf_" + s].constraints.new("IK")
            c.target, c.pole_target, c.chain_count = self.tgt[s], self.pole[s], 2
            r = arm.pose.bones["foot_" + s].constraints.new("COPY_ROTATION")
            r.target = self.tgt[s]
        self.cons = [c for pb in arm.pose.bones for c in pb.constraints]

    def empty(self, name):
        e = bpy.data.objects.new(name, None)
        bpy.context.scene.collection.objects.link(e)
        return e

    def basis(self, name, q):
        Mr = self.rest[name].to_3x3()
        return (Mr.inverted() @ q.to_matrix() @ Mr).to_quaternion()

    def solve(self, Q, P):
        """Q: 뼈 → rest 축 회전(Quaternion). P: pelvis_loc, foot_l, foot_r. 반환: 뼈 → (quaternion, location)."""
        arm = self.arm
        for pb in arm.pose.bones:
            pb.rotation_quaternion, pb.location = (1, 0, 0, 0), (0, 0, 0)
        for name, q in Q.items():
            arm.pose.bones[name].rotation_quaternion = self.basis(name, q)
        arm.pose.bones["pelvis"].location = self.rest["pelvis"].to_3x3().inverted() @ Vector(P["pelvis_loc"])
        for s in "lr":
            dx, dy, dz, rz = P["foot_" + s]
            m = Matrix.Rotation(D(rz), 4, "Z") @ (arm.matrix_world @ self.rest["foot_" + s]).to_3x3().to_4x4()
            m.translation = self.ankle[s] + Vector((dx, dy, dz))
            self.tgt[s].matrix_world = m
            fwd = Matrix.Rotation(D(rz), 3, "Z") @ Vector((0, -1, 0))
            self.pole[s].location = self.ankle[s] + Vector((dx, dy, 0.5)) + fwd * 0.8
        for c in self.cons:
            c.enabled = True
        bpy.context.view_layer.update()
        M = {b: arm.pose.bones[b].matrix.copy() for b in LEG}
        for c in self.cons:
            c.enabled = False
        bpy.context.view_layer.update()
        for b in LEG:
            arm.pose.bones[b].matrix = M[b]
            bpy.context.view_layer.update()
        return {pb.name: (tuple(pb.rotation_quaternion), tuple(pb.location)) for pb in arm.pose.bones}

    def calibrate(self, poses):
        """무릎이 발끝 쪽을 보게 하는 pole_angle 을 고른다. 모든 자세에서 무릎이 발끝 쪽으로 나온 거리의 합이 큰 쪽."""
        best = {}
        for s in "lr":
            scores = []
            for ang in (-90, 0, 90, 180):
                self.arm.pose.bones["calf_" + s].constraints[0].pole_angle = D(ang)
                total = 0.0
                for F in poses:
                    self.solve(*split(F))
                    pbs = self.arm.pose.bones
                    hip, knee, ank = pbs["thigh_" + s].head, pbs["calf_" + s].head, pbs["calf_" + s].tail
                    fwd = Matrix.Rotation(D(F["foot_" + s][3]), 3, "Z") @ Vector((0, -1, 0))
                    total += (knee - (hip + ank) / 2).dot(fwd)
                scores.append((total, ang))
            best[s] = max(scores)[1]
            self.arm.pose.bones["calf_" + s].constraints[0].pole_angle = D(best[s])
        return best

    def clear(self):
        for pb in self.arm.pose.bones:
            for c in list(pb.constraints):
                pb.constraints.remove(c)
        for e in list(self.tgt.values()) + list(self.pole.values()):
            bpy.data.objects.remove(e)


SPECIAL = ("pelvis_loc", "foot_l", "foot_r")


def split(F, Fb=None, t=0.0):
    """편 자세(또는 두 자세와 비율)를 회전과 위치로 나눈다. 회전은 slerp, 위치는 lerp."""
    if Fb is None:
        Fb = F
    names = (set(F) | set(Fb)) - set(SPECIAL)
    Q = {n: qmix(q_of(F.get(n, [0, 0, 0])), q_of(Fb.get(n, [0, 0, 0])), t) for n in names}
    P = {k: lerp(F[k], Fb[k], t) for k in SPECIAL}
    return Q, P


def make(rig_path, pose_path, out):
    spec = json.load(open(pose_path))
    fps = spec.get("fps", 30)
    arm, _ = load(rig_path, fps)
    poses = {k: flat(v) for k, v in spec["poses"].items()}
    rig = Rig(arm)
    print("pole", rig.calibrate(list(poses.values())))
    solved = {k: rig.solve(*split(F)) for k, F in poses.items()}
    clips = []
    for c in spec["clips"]:
        n = c["frames"]
        frames = []
        if "pose" in c:
            base = poses[c["pose"]]
            for f in range(n if c.get("loop") else n + 1):
                if f == 0 and not c.get("waves"):
                    frames.append(solved[c["pose"]])
                    continue
                F = {k: list(v) for k, v in base.items()}
                for w in c.get("waves", []):
                    ph, cyc = w.get("phase", 0.0), w.get("cycles", 1)
                    k = math.sin(2 * math.pi * cyc * f / n + ph) - math.sin(ph)
                    cur = F.get(w["key"], [0] * len(w["amp"]))
                    F[w["key"]] = [a + b * k for a, b in zip(cur, w["amp"])]
                frames.append(solved[c["pose"]] if f == 0 else rig.solve(*split(F)))
            if c.get("loop"):
                frames.append(frames[0])
        else:
            A, B = poses[c["from"]], poses[c["to"]]
            steps = {s["foot"]: s for s in c.get("steps", [])}
            for f in range(n + 1):
                if f in (0, n):
                    frames.append(solved[c["from"] if f == 0 else c["to"]])
                    continue
                t = f / n
                Q, P = split(A, B, ease(c.get("ease", "in_out"), t))
                for s, st in steps.items():
                    u = min(max((t - st["start"]) / (st["end"] - st["start"]), 0.0), 1.0)
                    dx, dy, dz, rz = lerp(A["foot_" + s], B["foot_" + s], u * u * (3 - 2 * u))
                    P["foot_" + s] = [dx, dy, dz + st.get("lift", 0.05) * math.sin(math.pi * u), rz]
                frames.append(rig.solve(Q, P))
        clips.append((c["name"], frames))
    rig.clear()
    ad = arm.animation_data_create()
    for name, frames in clips:
        ad.action = None
        for f, pose in enumerate(frames):
            for b, (q, loc) in pose.items():
                pb = arm.pose.bones[b]
                pb.rotation_quaternion, pb.location = q, loc
                pb.keyframe_insert("rotation_quaternion", frame=f, group=b)
                pb.keyframe_insert("location", frame=f, group=b)
        act = ad.action
        act.name = name
        # glTF 내보내기(NLA_TRACKS)는 트랙 이름을 애니메이션 이름으로 쓴다.
        track = ad.nla_tracks.new()
        track.name = name
        track.strips.new(name, 0, act)
    ad.action = None
    for pb in arm.pose.bones:
        pb.rotation_quaternion, pb.location = (1, 0, 0, 0), (0, 0, 0)
    bpy.ops.export_scene.gltf(filepath=out, export_format="GLB", export_animation_mode="NLA_TRACKS")
    print("anim", out, "clips", " ".join(f"{n}:{len(fr) - 1}" for n, fr in clips))


def use(arm, name):
    ad = arm.animation_data
    for t in ad.nla_tracks:
        t.mute = True
    a = bpy.data.actions[name]
    ad.action = a
    if len(a.slots):
        ad.action_slot = a.slots[0]
    return int(a.frame_range[0]), int(a.frame_range[1])


def check(anim_path, pose_path):
    spec = json.load(open(pose_path))
    lim = {**DEFAULT_CHECK, **spec.get("check", {})}
    arm, body = load(anim_path, spec.get("fps", 30))
    sc = bpy.context.scene
    fails, ends, slides, floors = [], {}, [], []
    for c in spec["clips"]:
        name = c["name"]
        if name not in bpy.data.actions:
            fails.append(f"{name}: clip missing")
            continue
        f0, f1 = use(arm, name)
        if f1 - f0 != c["frames"]:
            fails.append(f"{name}: {f1 - f0} frames, expected {c['frames']}")
        feet, lows, mats = {"l": [], "r": []}, [], []
        for f in range(f0, f1 + 1):
            sc.frame_set(f)
            for s in "lr":
                feet[s].append(arm.matrix_world @ arm.pose.bones["foot_" + s].head)
            ev = body.evaluated_get(bpy.context.evaluated_depsgraph_get())
            lows.append(min((body.matrix_world @ v.co).z for v in ev.data.vertices))
            mats.append([pb.matrix.copy() for pb in arm.pose.bones])
        ends[name] = (mats[0], mats[-1])
        lo, hi = min(lows), max(lows)
        floors += [lo, hi]
        line = f"clip {name} frames {f1 - f0} floor {lo:.3f}..{hi:.3f}"
        if lo < -lim["floor"]:
            fails.append(f"{name}: mesh {-lo:.3f} m under the floor")
        if hi > lim["floor"]:
            fails.append(f"{name}: both feet {hi:.3f} m off the floor")
        if "pose" in c:
            slide = max(max((p - feet[s][0]).length for p in feet[s]) for s in "lr")
            slides.append(slide)
            line += f" foot_slide {slide:.3f}"
            if slide > lim["foot_slide"]:
                fails.append(f"{name}: feet slide {slide:.3f} m")
        if c.get("loop"):
            d = mdiff(mats[0], mats[-1])
            line += f" loop {d:.4f}"
            if d > lim["seam"]:
                fails.append(f"{name}: loop gap {d:.4f}")
        print(line)
    for a, b in zip(spec["clips"], spec["clips"][1:]):
        end_a = a.get("to", a.get("pose"))
        start_b = b.get("from", b.get("pose"))
        if end_a == start_b and a["name"] in ends and b["name"] in ends:
            d = mdiff(ends[a["name"]][1], ends[b["name"]][0])
            print(f"seam {a['name']}->{b['name']} {d:.4f}")
            if d > lim["seam"]:
                fails.append(f"seam {a['name']}->{b['name']} {d:.4f}")
    if fails:
        print("anim check failed: " + "; ".join(fails))
        sys.exit(1)
    print(f"anim check ok: {len(spec['clips'])} clips, foot slide {max(slides, default=0):.3f} m, floor {min(floors):.3f}..{max(floors):.3f} m")


def mdiff(a, b):
    return max(abs(x - y) for m0, m1 in zip(a, b) for r0, r1 in zip(m0, m1) for x, y in zip(r0, r1))


def render_setup(body, w, h):
    import numpy as np  # noqa: F401  (Blender 에 들어 있다)

    sc = bpy.context.scene
    sc.render.engine = "BLENDER_WORKBENCH"
    sc.display.shading.light = "STUDIO"
    sc.display.shading.color_type = "OBJECT"
    sc.display.shading.show_shadows = True
    sc.render.resolution_x, sc.render.resolution_y = w, h
    bpy.ops.mesh.primitive_plane_add(size=4)
    bpy.context.object.color = (0.35, 0.45, 0.6, 1)
    body.color = (0.85, 0.85, 0.85, 1)
    cam = bpy.data.objects.new("cam", bpy.data.cameras.new("cam"))
    sc.collection.objects.link(cam)
    sc.camera = cam
    cam.data.type = "ORTHO"
    return cam


def shot(cam, pos, look, scale, path):
    import numpy as np

    cam.location = pos
    cam.rotation_euler = (Vector(look) - Vector(pos)).to_track_quat("-Z", "Y").to_euler()
    cam.data.ortho_scale = scale
    bpy.context.scene.render.filepath = path
    bpy.ops.render.render(write_still=True)
    im = bpy.data.images.load(path)
    px = np.array(im.pixels[:]).reshape(im.size[1], im.size[0], 4)
    bpy.data.images.remove(im)
    return px


def save_png(px, path):
    h, w = px.shape[:2]
    img = bpy.data.images.new(os.path.basename(path), w, h)
    img.pixels = px.ravel()
    img.filepath_raw, img.file_format = path, "PNG"
    img.save()


def sheet(anim_path, pose_path, out, ref=None):
    import numpy as np

    spec = json.load(open(pose_path))
    arm, body = load(anim_path, spec.get("fps", 30))
    height = max((body.matrix_world @ Vector(c)).z for c in body.bound_box)
    look, scale = [0, 0, height * 0.5], height * 1.3
    views = {**DEFAULT_VIEWS, **spec.get("views", {})}
    tmp = out + ".tile.png"
    samples = []
    for c in spec["clips"]:
        n = c["frames"]
        samples += [(c["name"], f) for f in ((n // 4, 3 * n // 4) if "pose" in c else (0, n // 2, n))]
    cam = render_setup(body, 300, 400)
    rows = []
    for v in views.values():
        row = []
        for name, f in samples:
            f0, _ = use(arm, name)
            bpy.context.scene.frame_set(f0 + f)
            row.append(shot(cam, v, look, scale, tmp))
        rows.append(np.concatenate(row, axis=1))
    save_png(np.concatenate(rows[::-1], axis=0), out)
    print("sheet", out, "views", " ".join(views), "frames", " ".join(f"{n}:{f}" for n, f in samples))
    if ref:
        # 참고 이미지 아래에 첫 유지 클립의 첫 프레임을 compare 시점들(기본: views.ref, 없으면 첫 시점)에서 그려 붙인다.
        # 자세 시트(앞·옆·3/4)를 참고로 주면 compare 를 ["front", "side", "three4"] 로 둔다.
        names = spec.get("compare", ["ref"] if "ref" in views else [next(iter(views))])
        im = bpy.data.images.load(ref)
        H = 600
        cw = max(1, round(im.size[0] * H / im.size[1] / len(names)))
        im.scale(cw * len(names), H)
        top = np.array(im.pixels[:]).reshape(H, cw * len(names), 4)
        hold = next(c for c in spec["clips"] if "pose" in c)
        f0, _ = use(arm, hold["name"])
        bpy.context.scene.frame_set(f0)
        sc = bpy.context.scene
        sc.render.resolution_x, sc.render.resolution_y = cw, H
        row = np.concatenate([shot(cam, views[n], look, scale * max(1.0, cw / H), tmp) for n in names], axis=1)
        cmp = out[:-4] + ".compare.png" if out.endswith(".png") else out + ".compare.png"
        save_png(np.concatenate([row, top], axis=0), cmp)
        print("compare", cmp, "views", " ".join(names))
    if os.path.exists(tmp):
        os.remove(tmp)


args = sys.argv[sys.argv.index("--") + 1 :]
if not args or args[0] not in ("make", "check", "sheet"):
    sys.exit("usage: blender_anim.py -- make <rig.glb> <pose.json> <out.glb> | check <anim.glb> <pose.json> | sheet <anim.glb> <pose.json> <out.png> [ref]")
{"make": make, "check": check, "sheet": sheet}[args[0]](*args[1:])
