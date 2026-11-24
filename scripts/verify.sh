#!/usr/bin/env bash
# 검증 명령. 마지막 줄이 "✓ verify passed" 이면 통과다. 구독 CLI(codex·claude)와 로컬 모델을 부르지 않는다.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --check
cargo clippy --all-targets -- -D warnings
rm -f target/tmp/fixture.glb target/tmp/sheet.glb target/tmp/moto4.glb target/tmp/model.glb target/tmp/model_low.glb target/tmp/man_rig.glb \
  target/tmp/man_anim.glb target/tmp/man_anim_sheet.png target/tmp/man_anim_sheet.compare.png target/tmp/man_anim_slide.glb target/tmp/pose_slide.json
python3 -m py_compile scripts/trellis_run.py scripts/blender_low.py scripts/blender_rig.py scripts/make_skeleton.py scripts/blender_anim.py
bash -n scripts/setup_model.sh
bash -n scripts/make_app.sh
cargo test
for glb in target/tmp/fixture.glb target/tmp/sheet.glb target/tmp/moto4.glb; do
  test -s "$glb"
  blender -b --factory-startup --python-exit-code 1 --python scripts/blender_check.py -- "$glb" 2000 \
    | grep -E "^blender check ok" || { echo "blender check failed: $glb"; exit 1; }
done

# 로컬 모델 경로의 메시: 삼각형 예산이 다르고 법선이 있다.
test -s target/tmp/model.glb
blender -b --factory-startup --python-exit-code 1 --python scripts/blender_check.py -- target/tmp/model.glb 50000 \
  | grep -E "^blender check ok" || { echo "blender check failed: target/tmp/model.glb"; exit 1; }
# 그 메시에서 Blender 가 만든 로우폴리 판.
test -s target/tmp/model_low.glb
blender -b --factory-startup --python-exit-code 1 --python scripts/blender_check.py -- target/tmp/model_low.glb 5000 \
  | grep -E "^blender check ok" || { echo "blender check failed: target/tmp/model_low.glb"; exit 1; }
# 사람형: 뼈대를 넣은 로우폴리 판. Blender 가 가져온 뒤에도 메시 1개, 재질 1개, 발바닥 원점이다.
test -s target/tmp/man_rig.glb
blender -b --factory-startup --python-exit-code 1 --python scripts/blender_check.py -- target/tmp/man_rig.glb 5000 \
  | grep -E "^blender check ok" || { echo "blender check failed: target/tmp/man_rig.glb"; exit 1; }

# 동작: 그 rig 에 fixture 자세의 enter · idle · exit 를 넣고, 다시 가져와 검사하고, 시트를 그린다.
anim() { blender -b --factory-startup --python-exit-code 1 --python scripts/blender_anim.py -- "$@"; }
anim make target/tmp/man_rig.glb test/fixtures/pose_guard.json target/tmp/man_anim.glb | grep -E "^anim "
anim check target/tmp/man_anim.glb test/fixtures/pose_guard.json | grep -E "^anim check ok"
anim sheet target/tmp/man_anim.glb test/fixtures/pose_guard.json target/tmp/man_anim_sheet.png test/fixtures/man_sheet.png | grep -E "^(sheet|compare) "
test -s target/tmp/man_anim_sheet.png
test -s target/tmp/man_anim_sheet.compare.png
# 검사가 실제로 잡는지: idle 에서 왼발을 흔드는 자세는 발 미끄러짐으로 실패해야 한다.
python3 -c 'import json; p = json.load(open("test/fixtures/pose_guard.json")); p["clips"][1]["waves"].append({"key": "foot_l", "amp": [0, 0.05, 0, 0]}); json.dump(p, open("target/tmp/pose_slide.json", "w"))'
anim make target/tmp/man_rig.glb target/tmp/pose_slide.json target/tmp/man_anim_slide.glb | grep -E "^anim "
if anim check target/tmp/man_anim_slide.glb target/tmp/pose_slide.json >target/tmp/anim_slide.log 2>&1; then
  echo "anim check missed a sliding foot"; exit 1
fi
grep -E "^anim check failed: .*idle: feet slide" target/tmp/anim_slide.log

# 바탕화면 앱이 있으면 이 코드로 다시 만든다. 없으면 만들지 않는다.
if [ -d "$HOME/Desktop/mytripo_public.app" ]; then
  mkdir -p target/tmp
  bash scripts/make_app.sh >target/tmp/make_app.log 2>&1 || { tail -20 target/tmp/make_app.log; echo "make_app failed"; exit 1; }
fi

echo "✓ verify passed"
