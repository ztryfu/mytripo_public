# mytripo_public

@architecture/INTENT.md

이 저장소의 에이전트 지침은 이 파일 하나다.

## 커맨드
- 검증: `bash scripts/verify.sh` — fmt, clippy(`-D warnings`), `cargo test`, `blender -b` 가져오기 검사, rig 동작 검사. 정상 출력의 마지막 세 줄:
  ```
  anim target/tmp/man_anim_slide.glb clips enter:14 idle:60 exit:18
  anim check failed: idle: feet slide 0.050 m
  ✓ verify passed
  ```
  `anim check failed` 줄은 검사가 일부러 망가뜨린 자세를 잡았다는 뜻이다. `blender check ok` 줄의 삼각형 수는 실행마다 몇 개 다를 수 있다(Blender 의 remesh).
- 로컬 모델 설치: `bash scripts/setup_model.sh`(TRELLIS.2, 환경 약 2 GB, 첫 생성 때 가중치 약 16 GB. Xcode Metal 툴체인, `hf auth login`, DINOv3 접근 승인이 먼저 필요하다). 위치는 `MYTRIPO_PUBLIC_MODEL_DIR`, 기본 `~/Documents/mytripo_public/model`.
- 실행: `cargo run --release`, 시트로 바로 시작: `cargo run --release -- --sheet-file <시트.png>`(구독을 쓴다)
- 바탕화면 앱: `bash scripts/make_app.sh` — `~/Desktop/mytripo_public.app` 을 만든다. 실행 파일을 번들에 복사한다. 번들이 있으면 검증 명령이 끝에서 같은 코드로 다시 만든다. 마지막 줄은 `app /Users/<사용자>/Desktop/mytripo_public.app`.
- 창 없이 한 번 돌리기(구독을 쓴다): `cargo run --release --example headless -- <사진>...`, `-- --sheet "<설명>" [참고 사진]...`, `-- --sheet-file <시트.png>`, `-- --edit <시트.png> "<수정 지시>"`, `-- --pose-sheet "<자세 설명>" <시트.png>`(같은 인물의 자세 시트 앞·옆·3/4, 동작의 참고 그림)
- 구독 없이 돌리기: 위 명령에 `--structure <구조 응답.json 또는 이전 작업 기록.json>` 을 붙인다. `--raw <시각.model.ply>` 를 더 붙이면 로컬 모델도 돌리지 않는다. `--seed <n>` 은 모델의 seed 다. `--model-input <png>` 는 모델에 넣는 그림을 저장한다(다른 모델과 비교할 때).
- 동작: `blender -b --factory-startup --python-exit-code 1 --python scripts/blender_anim.py -- make <rig.glb> <pose.json> <out.glb>`, 같은 명령의 `check <anim.glb> <pose.json>`(통과하면 `anim check ok: …`), `sheet <anim.glb> <pose.json> <out.png> [참고 이미지]`(참고 이미지를 주면 그 아래에 `compare` 시점들의 렌더를 붙인 `.compare.png`). 자세 JSON 의 형식과 축은 스크립트 머리 주석이 정본이다. 예는 `test/fixtures/pose_guard.json`.
- fixture 사진·시트 다시 렌더: `blender -b --factory-startup --python test/fixtures/make_fixtures.py -- test/fixtures`

작업을 완료로 보고하기 전에 검증 명령을 돌리고 출력을 붙인다. 테스트가 실패하면 테스트가 아니라 코드를 고친다.

## 구조
- `src/` — `mask` 배경 제거, `sheet` 시트 프롬프트·자르기, `kit` 사물의 부품 조립, `hull` 사물의 실루엣 교집합(부품 목록이 없을 때), `model` 로컬 모델(TRELLIS.2) 호출과 칸 맞추기, `lowpoly` 모델 메시의 로우폴리 판(Blender), `rig` 사람형에 기준 마네킹의 뼈대 넣기(Blender), `codex` 시트 생성(image_gen), `claude` 구조 읽기, `cli` 하위 프로세스, `structure` 응답 타입, `pipeline` 단면 고리 로프트·저장, `mesh` 고리·튜브, `raster` 직교 실루엣·미리보기, `glb` 출력, `history` 이전 작업 기록, `app` 창.
- `schema/` — 구조 읽기의 JSON schema·프롬프트, 시트 그림의 방향 블록(`style.txt`), 기준 마네킹의 뼈대(`mannequin_skeleton.json`, `scripts/make_skeleton.py` 가 만든다). 앱이 `include_str!` 로 넣는다.
- `test/fixtures/` — Blender 로 렌더한 사진 3장과 시트 1장, codex 가 생성한 시트들(비옷 아이, 오토바이 3칸·4칸·사이드백·빨강, 좀비), 각각의 claude 응답 JSON, 렌더 스크립트, SF 오토바이 시트와 그 모델 메시(`moto_sf_model.ply` 는 `1024_cascade` + remesh, `moto_sf_512_model.ply` 는 `512`), 사람 시트와 그 모델 메시(`man_model.ply`). 테스트는 `target/tmp/<tag>_preview.png`·`*_iou_*.png` 를 남긴다.
- `scripts/` — `verify.sh`, `blender_check.py`, `setup_model.sh` 로컬 모델 설치, `trellis_run.py` 앱이 부르는 모델 실행 스크립트, `blender_low.py` 로우폴리 스크립트, `blender_rig.py` 뼈대 스크립트(앱이 셋을 `include_str!` 로 넣는다), `blender_anim.py` rig 에 동작 넣기 · 검사 · 시트(앱 밖에서 돈다), `make_app.sh` 바탕화면 앱 번들.
- `assets/icon.png` — 앱 아이콘. 창과 번들의 `.icns` 가 같은 PNG 를 쓴다.
- `architecture/INTENT.md` 는 제품의 현재 상태, `architecture/decisions/` 는 확정된 결정의 근거를 담는다.

## 컨벤션
- 검증 명령과 테스트는 구독 CLI(`codex` `claude`)와 로컬 모델을 부르지 않는다. Blender 는 부른다. AI 응답이 필요한 테스트는 `test/fixtures/structure.json` 을, 모델 메시가 필요한 테스트는 `test/fixtures/moto_sf_model.ply` `moto_sf_512_model.ply` `man_model.ply` 를 쓴다.
- `scripts/trellis_run.py` 나 `src/model.rs` 의 모델 입력을 바꾸면 headless 예제로 모델을 다시 돌려 `moto_sf_model.ply` 를 다시 저장한다.
- `schema/` 를 바꾸면 headless 예제로 fixture 사진과 시트들의 claude 응답을 다시 받아 `test/fixtures/*.json` 을 다시 저장한다.
- 장기 설계 결정은 `architecture/decisions/` 에 근거를 남긴다.

## 에이전트가 틀리는 것
- 같은 실수가 두 번째 나오면 검증 명령이 잡는 검사(린터, 타입, 테스트, 스크립트)로 만든다. 검사로 만들 수 없을 때만 여기에 규칙 한 줄.
