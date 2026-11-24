# 앱이 부르는 TRELLIS.2 실행 스크립트. src/model.rs 가 include_str! 로 넣고 작업 폴더에 써서 실행한다.
# 사용: python trellis_run.py <trellis-mac 폴더> <출력 ply> <입력 RGBA png> [seed]
# seed 는 모델이 가려진 쪽을 추정할 때 쓰는 난수의 시작 값이다. 같은 seed 는 거의 같은 형태를 낸다.
# 입력 그림의 alpha 가 피사체다. 출력은 binary little-endian PLY(정점 float x y z, 삼각형 면), Z-up 이다.
# 환경변수: MYTRIPO_PUBLIC_PIPELINE(기본 1024_cascade, 빠른 판은 512), MYTRIPO_PUBLIC_REMESH(기본 1, 0 이면 dual contouring remesh 를 끈다).
import os
import sys
import time

os.environ.setdefault("PYTORCH_ENABLE_MPS_FALLBACK", "1")
os.environ.setdefault("ATTN_BACKEND", "sdpa")
os.environ.setdefault("SPARSE_ATTN_BACKEND", "sdpa")
try:
    import flex_gemm  # noqa: F401

    os.environ.setdefault("SPARSE_CONV_BACKEND", "flex_gemm")
except (ImportError, RuntimeError):
    os.environ.setdefault("SPARSE_CONV_BACKEND", "none")

root, dst, src = sys.argv[1:4]
SEED = int(sys.argv[4]) if len(sys.argv) > 4 else 42
PIPELINE = os.environ.get("MYTRIPO_PUBLIC_PIPELINE", "1024_cascade")
REMESH = os.environ.get("MYTRIPO_PUBLIC_REMESH", "1") == "1"
# 정리(o_voxel)에 넘기기 전에 줄이는 삼각형 수. Metal BVH 는 이보다 큰 메시에서 불안정하다.
FACES = 200_000
sys.path.insert(0, os.path.join(root, "TRELLIS.2"))
sys.path.append(os.path.join(root, "stubs"))

import fast_simplification
import numpy as np
import torch
import trimesh
from PIL import Image

import trellis2.pipelines.rembg as rembg


class NoRembg:
    """배경 제거 모델(RMBG-2.0, CC BY-NC)을 내려받지 않는다. 입력은 이미 alpha 가 있다."""

    def __init__(self, **kw):
        pass

    def to(self, *a, **k):
        return self

    def cpu(self):
        return self

    def __call__(self, img):
        raise RuntimeError("the input image needs an alpha channel")


rembg.BiRefNet = NoRembg
import o_voxel.postprocess as post
from trellis2.pipelines.trellis2_image_to_3d import Trellis2ImageTo3DPipeline

img = Image.open(src).convert("RGBA")
if np.asarray(img)[..., 3].max() == 0:
    sys.exit("the input image has no subject")

t0 = time.time()
pipe = Trellis2ImageTo3DPipeline.from_pretrained("microsoft/TRELLIS.2-4B")
pipe.to(torch.device("mps"))
print(f"loaded {time.time() - t0:.1f}s, conv {os.environ['SPARSE_CONV_BACKEND']}, postprocess {post._BACKEND}", flush=True)

t1 = time.time()
m = pipe.run(img, seed=SEED, pipeline_type=PIPELINE)
m = m[0] if isinstance(m, list) else m
v, f = m.vertices.cpu().numpy(), m.faces.cpu().numpy()
if len(f) == 0:
    sys.exit("the model returned an empty mesh")
if len(f) > FACES:
    v, f = fast_simplification.simplify(v, f, 1.0 - FACES / len(f))
print(f"generated {time.time() - t1:.1f}s ({PIPELINE}): {len(v)} vertices, {len(f)} faces", flush=True)

t2 = time.time()
# 원본과 같은 정리: 면 방향 통일, 비다양체 수리, 구멍 메우기, remesh 면 narrow-band dual contouring.
glb = post.to_glb(
    vertices=torch.from_numpy(np.ascontiguousarray(v)).float(),
    faces=torch.from_numpy(f.astype("int32")),
    attr_volume=m.attrs.cpu(),
    coords=m.coords.cpu(),
    attr_layout=m.layout,
    voxel_size=m.voxel_size,
    aabb=[[-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]],
    decimation_target=min(FACES, len(f)),
    texture_size=512,
    remesh=REMESH,
    verbose=False,
)
geo = glb.to_geometry() if isinstance(glb, trimesh.Scene) else glb
tm = trimesh.Trimesh(geo.vertices, geo.faces, process=False)
# 정리는 UV 경계에서 정점을 나눈다. 같은 자리의 정점을 합치지 않으면 앱이 조각을 떠 있는 것으로 보고 지운다.
tm.merge_vertices(digits_vertex=6)
v = np.asarray(tm.vertices)
# 모델은 Y-up 이다. 앱이 읽는 Z-up 으로 돌린다.
v = np.stack([v[:, 0], -v[:, 2], v[:, 1]], axis=1).astype("<f4")
f = np.ascontiguousarray(tm.faces).astype("<i4")
print(f"cleaned {time.time() - t2:.1f}s (remesh {int(REMESH)}): {len(v)} vertices, {len(f)} faces", flush=True)

faces = np.empty(len(f), dtype=[("n", "u1"), ("i", "<i4", (3,))])
faces["n"] = 3
faces["i"] = f
with open(dst, "wb") as out:
    out.write(
        (
            "ply\nformat binary_little_endian 1.0\n"
            f"element vertex {len(v)}\nproperty float x\nproperty float y\nproperty float z\n"
            f"element face {len(f)}\nproperty list uchar int vertex_indices\nend_header\n"
        ).encode()
    )
    v.tofile(out)
    faces.tofile(out)
print(f"wrote {dst}", flush=True)
