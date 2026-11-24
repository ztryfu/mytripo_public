#!/usr/bin/env bash
# 로컬 image-to-3D 모델(TRELLIS.2, Apple Silicon 판)을 설치한다. 앱은 이 환경이 있으면 형태를 모델로 만든다.
# 사용: bash scripts/setup_model.sh
# 위치: MYTRIPO_PUBLIC_MODEL_DIR (기본 ~/Documents/mytripo_public/model). 환경 약 2 GB.
# 가중치 약 16 GB 는 첫 생성 때 Hugging Face 캐시(~/.cache/huggingface)에 내려받는다.
# 먼저 할 일:
#   1. xcodebuild -downloadComponent MetalToolchain   (Metal 백엔드 빌드에 필요)
#   2. hf auth login                                    (Hugging Face 로그인)
#   3. https://huggingface.co/facebook/dinov3-vitl16-pretrain-lvd1689m 에서 접근 승인을 받는다
set -euo pipefail

DIR="${MYTRIPO_PUBLIC_MODEL_DIR:-$HOME/Documents/mytripo_public/model}"
# shivampkumar/trellis-mac (MIT): TRELLIS.2 를 PyTorch MPS 로 옮긴 판. 설치 스크립트와 패치를 쓴다.
PORT=d58628f
# 아래 커밋은 이 조합으로 생성을 확인한 값이다. trellis-mac 의 setup.sh 는 고정하지 않고 받으므로 여기서 먼저 받는다.
TRELLIS=75fbf0183001ed9876c8dbb35de6b68552ee08bd
DEPS="
https://github.com/EasternJournalist/utils3d.git utils3d 9a4eb15e4021b67b12c460c7057d642626897ec8
https://github.com/pedronaugusto/mtlbvh.git mtlbvh 011896cadb12349d1f4e906af41ff96aa5b5fcb7
https://github.com/pedronaugusto/mtldiffrast.git mtldiffrast db3186be168a495f43ea095f7311a18a4de0a74c
https://github.com/pedronaugusto/mtlmesh.git mtlmesh 4a46faae8ab6def6f308c499e22a23020cdacd5b
https://github.com/pedronaugusto/mtlgemm.git mtlgemm c2e5d3fa15152bc4aef65773cab1ded5a1855d38
https://github.com/pedronaugusto/trellis2-apple.git trellis2-apple c873bde81ebe1e56535c6caddb4b21267364a923
"

[ "$(uname -sm)" = "Darwin arm64" ] || { echo "Apple Silicon Mac 이 필요합니다"; exit 1; }
xcrun -sdk macosx metal --version >/dev/null 2>&1 \
  || { echo "Metal 툴체인이 없습니다: xcodebuild -downloadComponent MetalToolchain"; exit 1; }

# 받은 커밋만 가져온다. 이미 있으면 건너뛴다.
fetch() {
  local url="$1" dir="$2" ref="$3"
  [ -d "$dir/.git" ] && return 0
  git init -q "$dir"
  git -C "$dir" remote add origin "$url"
  git -C "$dir" fetch -q --depth 1 origin "$ref" || git -C "$dir" fetch -q origin
  git -C "$dir" checkout -q "$ref"
}

mkdir -p "$DIR"
cd "$DIR"
if [ ! -d trellis-mac/.git ]; then
  git clone -q https://github.com/shivampkumar/trellis-mac.git
  git -C trellis-mac checkout -q "$PORT"
fi
cd trellis-mac
mkdir -p deps
echo "$DEPS" | while read -r url dir ref; do
  [ -n "$url" ] || continue
  fetch "$url" "deps/$dir" "$ref"
done
fetch https://github.com/microsoft/TRELLIS.2.git TRELLIS.2 "$TRELLIS"
git -C deps/trellis2-apple submodule update -q --init o-voxel/third_party/eigen
# torch 2.15 의 헤더는 C++20 을 요구한다. Metal 백엔드의 빌드 설정은 C++17 이다.
sed -i '' 's/-std=c++17/-std=c++20/g' \
  deps/mtlbvh/setup.py deps/mtldiffrast/setup.py deps/mtlmesh/setup.py deps/mtlgemm/setup.py \
  deps/trellis2-apple/o-voxel/setup.py

if [ ! -x .venv/bin/python ]; then
  if command -v uv >/dev/null; then
    uv venv -q .venv --python 3.11
  elif command -v python3.11 >/dev/null; then
    python3.11 -m venv .venv
  else
    echo "python 3.11 이 필요합니다 (brew install uv 또는 brew install python@3.11)"; exit 1
  fi
fi
# 생성을 확인한 패키지 버전으로 고정한다.
cat > constraints.txt <<'EOF'
accelerate==1.16.0
easydict==1.14
fast-simplification==0.2.0
huggingface-hub==2.4.0
imageio==2.38.2
kornia==0.9.2
numpy==2.4.6
opencv-python-headless==5.1.0.95
pillow==13.0.0
plyfile==1.1.5
pybind11==3.1.0
safetensors==0.8.0
scipy==1.17.1
setuptools==84.2.1
timm==1.0.31
tokenizers==0.23.3
torch==2.15.0
torchaudio==2.11.1
torchvision==0.29.2
tqdm==4.70.1
transformers==5.24.0
trimesh==5.2.2
wheel==0.48.0
xatlas==0.0.11
zstandard==0.25.0
EOF
export UV_CONSTRAINT="$PWD/constraints.txt" PIP_CONSTRAINT="$PWD/constraints.txt"
bash setup.sh

.venv/bin/python - <<'EOF'
import sys
sys.path.append("stubs")
import torch
import cumesh, flex_gemm  # noqa: F401
import o_voxel.postprocess as post
if post._BACKEND != "metal":
    sys.exit(f"Metal 백엔드가 빌드되지 않았습니다 (postprocess {post._BACKEND}). 위의 빌드 오류를 확인하세요")
try:
    from huggingface_hub import hf_hub_download
    hf_hub_download("facebook/dinov3-vitl16-pretrain-lvd1689m", "config.json")
except Exception as e:
    sys.exit(f"DINOv3 가중치에 접근하지 못했습니다 ({type(e).__name__}). hf auth login 을 하고 "
             "https://huggingface.co/facebook/dinov3-vitl16-pretrain-lvd1689m 에서 접근 승인을 받으세요")
print(f"✓ model ready: torch {torch.__version__}, mps {torch.backends.mps.is_available()}, postprocess {post._BACKEND}")
EOF
