//! test/fixtures 의 사진 3장과 저장한 AI 응답으로 메시를 만들고 검사한다. 구독 CLI 를 부르지 않는다.

use std::path::{Path, PathBuf};

use mytripo_public::cli::Cancel;
use mytripo_public::lowpoly;
use mytripo_public::model;
use mytripo_public::pipeline::Photo;
use mytripo_public::pipeline::{self, DEFAULT_BUDGET, IOU_MIN, Slots};
use mytripo_public::raster;
use mytripo_public::rig;
use mytripo_public::sheet;
use mytripo_public::structure::{Structure, Subject, View};

/// 사람이 아닌 대상의 IoU 합격 기준값. 부품 조립은 단면을 볼록하게 채워 실루엣 교집합보다 덜 겹친다(decisions/0005).
const OBJECT_IOU_MIN: f32 = 0.85;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures")
}

#[test]
fn fixture_mesh_matches_silhouettes_within_budget() {
    let dir = fixtures();
    let photos: Vec<_> = ["photo_a.png", "photo_b.png", "photo_c.png"]
        .iter()
        .map(|n| pipeline::load_photo(&dir.join(n)).unwrap())
        .collect();
    let json = std::fs::read_to_string(dir.join("structure.json")).unwrap();
    let st = Structure::parse(&json, photos.len()).unwrap();
    let slots = Slots::from_structure(&st, photos.len());
    assert_eq!(
        slots,
        Slots {
            front: Some(1),
            side: Some(0),
            back: Some(2),
            top: None,
        },
        "AI 판별"
    );

    let model = pipeline::build(&photos, &slots, &st, DEFAULT_BUDGET).unwrap();
    let tris = model.mesh.triangle_count();
    println!("triangles {tris} / {DEFAULT_BUDGET}");
    assert!(tris <= DEFAULT_BUDGET);
    assert_eq!(model.frames.len(), 3);

    let mut ious = Vec::new();
    for (i, frame) in &model.frames {
        let m = &photos[*i].mask;
        let sil = raster::silhouette(&model.mesh, frame, m.w, m.h);
        let iou = m.iou(&sil);
        println!("{:?} IoU {iou:.3}", frame.view);
        // 겹침 그림: 빨강 = 사진에만, 파랑 = 메시에만, 흰색 = 둘 다
        let overlay = image::RgbImage::from_fn(m.w as u32, m.h as u32, |x, y| {
            let (a, b) = (m.get(x as i64, y as i64), sil.get(x as i64, y as i64));
            image::Rgb(match (a, b) {
                (true, true) => [235, 235, 235],
                (true, false) => [220, 40, 40],
                (false, true) => [40, 90, 230],
                _ => [20, 20, 20],
            })
        });
        overlay
            .save(Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("iou_{:?}.png", frame.view)))
            .unwrap();
        ious.push((frame.view, iou));
    }
    let preview = raster::render(&model.mesh, &raster::Camera::framing(&model.mesh), 600, 800);
    image::RgbaImage::from_raw(600, 800, preview)
        .unwrap()
        .save(Path::new(env!("CARGO_TARGET_TMPDIR")).join("preview.png"))
        .unwrap();
    for (view, iou) in ious {
        assert!(iou >= IOU_MIN, "{view:?} IoU {iou:.3} < {IOU_MIN}");
    }

    // 발바닥 원점, Y-up, 미터
    let (lo, hi) = model.mesh.bounds();
    assert!(lo.y.abs() < 0.02, "min y {}", lo.y);
    assert!((hi.y - st.height_m).abs() < 0.05, "max y {}", hi.y);

    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("fixture.glb");
    std::fs::write(&out, mytripo_public::glb::encode(&model.mesh)).unwrap();
    println!("wrote {}", out.display());
}

#[test]
fn moving_photos_rebuilds_with_the_new_slots() {
    let dir = fixtures();
    let photos: Vec<_> = ["photo_a.png", "photo_b.png", "photo_c.png"]
        .iter()
        .map(|n| pipeline::load_photo(&dir.join(n)).unwrap())
        .collect();
    let st = Structure::parse(
        &std::fs::read_to_string(dir.join("structure.json")).unwrap(),
        3,
    )
    .unwrap();
    let mut slots = Slots::from_structure(&st, 3);
    // 앞 사진을 뒤 칸으로 옮기면 뒤 사진이 앞 칸으로 온다. 뼈대는 뒤 사진 기준으로 다시 만든다.
    assert!(slots.move_photo(1, View::Back));
    assert_eq!(slots.front, Some(2));
    let model = pipeline::build(&photos, &slots, &st, DEFAULT_BUDGET).unwrap();
    assert!(model.mesh.triangle_count() <= DEFAULT_BUDGET);
    // 예산을 줄이면 삼각형도 줄어든다.
    let small = pipeline::build(&photos, &slots, &st, 900).unwrap();
    assert!(small.mesh.triangle_count() <= 900);
}

/// 메시를 만들고 세 방향 IoU 와 예산을 검사한다. 겹침 그림을 target/tmp/<tag>_iou_<view>.png 에 남긴다.
fn check(tag: &str, photos: &[Photo], st: &Structure) -> pipeline::Model {
    let min = match st.subject {
        Subject::Human => IOU_MIN,
        Subject::Other => OBJECT_IOU_MIN,
    };
    let slots = Slots::for_photos(st, photos.len());
    let model = pipeline::build(photos, &slots, st, DEFAULT_BUDGET).unwrap();
    assert!(model.mesh.triangle_count() <= DEFAULT_BUDGET);
    assert_eq!(model.frames.len(), photos.len());
    for (i, frame) in &model.frames {
        let m = &photos[*i].mask;
        let sil = raster::silhouette(&model.mesh, frame, m.w, m.h);
        let iou = m.iou(&sil);
        println!("{tag} {:?} IoU {iou:.3}", frame.view);
        assert!(iou >= min, "{tag} {:?} IoU {iou:.3} < {min}", frame.view);
    }
    // 미리보기 그림: 앞 3/4, 옆, 위에서 본 3/4
    let base = raster::Camera::framing(&model.mesh);
    let mut sheet = image::RgbaImage::new(1200, 400);
    for (k, (yaw, pitch)) in [(0.5, 0.15), (std::f32::consts::FRAC_PI_2, 0.05), (2.3, 0.7)]
        .into_iter()
        .enumerate()
    {
        let cam = raster::Camera { yaw, pitch, ..base };
        let rgba = raster::render(&model.mesh, &cam, 400, 400);
        let img = image::RgbaImage::from_raw(400, 400, rgba).unwrap();
        image::imageops::overlay(&mut sheet, &img, k as i64 * 400, 0);
    }
    sheet
        .save(Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{tag}_preview.png")))
        .unwrap();
    model
}

#[test]
fn turnaround_sheet_splits_into_three_views_and_builds() {
    let dir = fixtures();
    let photos = sheet::load(&dir.join("sheet.png")).unwrap();
    assert_eq!(photos.len(), 3);
    let json = std::fs::read_to_string(dir.join("sheet_structure.json")).unwrap();
    let st = Structure::parse(&json, 3).unwrap();
    // 시트는 왼쪽부터 앞·옆·뒤다.
    assert_eq!(
        Slots::from_structure(&st, 3),
        Slots {
            front: Some(0),
            side: Some(1),
            back: Some(2),
            top: None,
        }
    );
    let mesh = check("sheet", &photos, &st).mesh;
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sheet.glb");
    std::fs::write(&out, mytripo_public::glb::encode(&mesh)).unwrap();
}

/// codex 가 실제로 생성한 시트(노란 비옷 아이)와 그 시트의 claude 응답. 생성 시트의 비율과 옷 모양을 회귀 검사한다.
#[test]
fn generated_sheet_builds_within_iou() {
    let dir = fixtures();
    let photos = sheet::load(&dir.join("generated_sheet.png")).unwrap();
    let json = std::fs::read_to_string(dir.join("generated_sheet_structure.json")).unwrap();
    let st = Structure::parse(&json, 3).unwrap();
    check("generated", &photos, &st);
}

/// 사람이 아닌 대상(codex 가 생성한 오토바이 시트). 세 방향 실루엣이 모두 맞아야 한다.
#[test]
fn non_human_sheet_matches_all_three_silhouettes() {
    let dir = fixtures();
    let photos = sheet::load(&dir.join("moto_sheet.png")).unwrap();
    let json = std::fs::read_to_string(dir.join("moto_sheet_structure.json")).unwrap();
    let st = Structure::parse(&json, 3).unwrap();
    let mesh = check("moto", &photos, &st).mesh;
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("moto.glb");
    std::fs::write(&out, mytripo_public::glb::encode(&mesh)).unwrap();
}

/// 4칸 시트(앞·옆·뒤·위)의 사물. 위 시점 실루엣까지 맞아야 바퀴가 머플러 폭으로 퍼지지 않는다.
#[test]
fn four_view_object_matches_the_top_silhouette() {
    let dir = fixtures();
    let photos = sheet::load(&dir.join("moto4_sheet.png")).unwrap();
    assert_eq!(photos.len(), 4);
    let json = std::fs::read_to_string(dir.join("moto4_sheet_structure.json")).unwrap();
    let st = Structure::parse(&json, 3).unwrap();
    let model = check("moto4", &photos, &st);
    assert!(model.frames.iter().any(|(_, f)| f.view == View::Top));
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("moto4.glb");
    std::fs::write(&out, mytripo_public::glb::encode(&model.mesh)).unwrap();
}

/// 손가락을 편 손(좀비 시트). 팔 끝 고리가 손가락 하나 폭으로 가늘어지면 안 된다.
#[test]
fn arm_tips_end_blunt_not_on_one_finger() {
    let dir = fixtures();
    let photos = sheet::load(&dir.join("zombie_sheet.png")).unwrap();
    let json = std::fs::read_to_string(dir.join("zombie_sheet_structure.json")).unwrap();
    let st = Structure::parse(&json, 3).unwrap();
    let model = check("zombie", &photos, &st);
    let arms: Vec<_> = model
        .rings
        .iter()
        .filter(|(k, _)| *k == mytripo_public::structure::PartKind::Arm)
        .collect();
    assert_eq!(arms.len(), 2);
    for (_, rings) in arms {
        let half = |r: &mytripo_public::mesh::Ring| (r.ru.0 + r.ru.1) / 2.0;
        let hand = &rings[rings.len() * 4 / 5..];
        let widest = hand.iter().map(half).fold(0.0, f32::max);
        let tip = half(rings.last().unwrap());
        println!("zombie arm tip {tip:.4} / hand {widest:.4}");
        assert!(
            tip >= 0.5 * widest,
            "tip {tip:.4} < half of hand {widest:.4}"
        );
    }
}

/// 메시를 정점을 공유하는 덩어리로 나누고 덩어리마다 경계 상자(최소, 최대)를 낸다.
fn shells(
    mesh: &mytripo_public::mesh::Mesh,
) -> Vec<(mytripo_public::geom::V3, mytripo_public::geom::V3)> {
    let n = mesh.positions.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for t in &mesh.tris {
        for k in 1..3 {
            let (a, b) = (
                root(&mut parent, t[0] as usize),
                root(&mut parent, t[k] as usize),
            );
            parent[a] = b;
        }
    }
    let mut boxes: std::collections::HashMap<
        usize,
        (mytripo_public::geom::V3, mytripo_public::geom::V3),
    > = Default::default();
    for t in &mesh.tris {
        for &i in t {
            let r = root(&mut parent, i as usize);
            let p = mesh.positions[i as usize];
            let e = boxes.entry(r).or_insert((p, p));
            e.0 = mytripo_public::geom::V3::new(e.0.x.min(p.x), e.0.y.min(p.y), e.0.z.min(p.z));
            e.1 = mytripo_public::geom::V3::new(e.1.x.max(p.x), e.1.y.max(p.y), e.1.z.max(p.z));
        }
    }
    boxes.into_values().collect()
}

/// 사람이 아닌 대상은 부품으로 조립한다. 오토바이의 두 바퀴는 차체와 떨어진 얇은 원기둥이어야 한다.
/// 실루엣 교집합은 바퀴를 차체와 한 덩어리의 판으로 만들었다.
#[test]
fn object_wheels_are_separate_thin_cylinders() {
    let dir = fixtures();
    let photos = sheet::load(&dir.join("moto_bags_sheet.png")).unwrap();
    let json = std::fs::read_to_string(dir.join("moto_bags_sheet_structure.json")).unwrap();
    let st = Structure::parse(&json, 3).unwrap();
    let model = check("moto_bags", &photos, &st);
    let wheels: Vec<_> = shells(&model.mesh)
        .into_iter()
        .filter(|(lo, hi)| {
            let (dx, dy, dz) = (hi.x - lo.x, hi.y - lo.y, hi.z - lo.z);
            dy > 0.3 * st.height_m && (dy - dz).abs() < 0.1 * dy && dx < 0.5 * dy
        })
        .collect();
    println!("wheel shells {wheels:?}");
    assert_eq!(wheels.len(), 2, "바퀴 모양의 덩어리 수");
}

/// 사용자가 만든 빨간 오토바이 시트. 왼쪽·오른쪽 사이드백은 옆 상자가 같다. 두 가방이 모두 생겨야 한다.
#[test]
fn twin_saddlebags_with_one_side_box_both_appear() {
    let dir = fixtures();
    let photos = sheet::load(&dir.join("moto_red_sheet.png")).unwrap();
    assert_eq!(photos.len(), 4);
    let json = std::fs::read_to_string(dir.join("moto_red_sheet_structure.json")).unwrap();
    let st = Structure::parse(&json, 3).unwrap();
    let model = check("moto_red", &photos, &st);
    let (lo, hi) = model.mesh.bounds();
    let (zmid, wide) = ((lo.z + hi.z) / 2.0, 0.5 * hi.x.max(-lo.x));
    // 뒤쪽 절반에서 몸통 바깥으로 나온 덩어리: 가방.
    let bags: Vec<_> = shells(&model.mesh)
        .into_iter()
        .filter(|(a, b)| (a.z + b.z) / 2.0 < zmid && ((a.x + b.x) / 2.0).abs() > wide)
        .map(|(a, b)| (a.x + b.x) / 2.0)
        .collect();
    println!("bag shells x {bags:?}");
    assert!(bags.iter().any(|&x| x > 0.0), "왼쪽 가방");
    assert!(bags.iter().any(|&x| x < 0.0), "오른쪽 가방");
}

/// 로컬 모델(TRELLIS.2)이 낸 메시를 시트의 칸에 맞춘다. 모델을 돌리지 않고 저장한 모델 메시를 쓴다.
/// 모델은 옆 칸 한 장만 본다. 옆은 잘 맞고, 앞·뒤·위는 모델이 추정한 형태라서 시트와 덜 겹친다(decisions/0006).
#[test]
fn model_mesh_fits_the_sheet_and_keeps_the_output_contract() {
    let dir = fixtures();
    let photos = sheet::load(&dir.join("moto_sf_sheet.png")).unwrap();
    assert_eq!(photos.len(), 4);
    let json = std::fs::read_to_string(dir.join("moto_sf_sheet_structure.json")).unwrap();
    let st = Structure::parse(&json, 3).unwrap();
    assert_eq!(st.subject, Subject::Other);
    let raw = model::read_ply(&std::fs::read(dir.join("moto_sf_model.ply")).unwrap()).unwrap();
    let slots = Slots::for_photos(&st, photos.len());
    let built = pipeline::build_with(&photos, &slots, &st, model::BUDGET, Some(&raw)).unwrap();
    let mesh = &built.mesh;
    let tris = mesh.triangle_count();
    println!("model triangles {tris} / {}", model::BUDGET);
    assert!(tris <= model::BUDGET && tris > DEFAULT_BUDGET);
    assert_eq!(mesh.normals.len(), mesh.positions.len());
    assert_eq!(built.frames.len(), 4);
    for (i, frame) in &built.frames {
        let iou = model::iou(mesh, &photos[*i], frame);
        println!("model {:?} IoU {iou:.3}", frame.view);
        let min = if frame.view == View::Side { 0.85 } else { 0.7 };
        assert!(iou >= min, "{:?} IoU {iou:.3} < {min}", frame.view);
    }
    // 발바닥 원점, Y-up, 미터. 법선은 바깥을 향한다.
    // 이 메시에서 거울은 몸체와 떨어진 조각이고 겉넓이 비율이 0.0011~0.0014 다. model::clean 이 MIN_SHARE(0.002) 미만이라 버린다.
    // 그래서 꼭대기가 시트의 키보다 0.10 m 낮다.
    let (lo, hi) = mesh.bounds();
    assert!(lo.y.abs() < 0.02, "min y {}", lo.y);
    assert!((hi.y - st.height_m).abs() < 0.12, "max y {}", hi.y);
    let volume: f32 = (0..tris)
        .map(|t| {
            let [a, b, c] = mesh.corners(t);
            a.dot(b.cross(c))
        })
        .sum();
    assert!(volume > 0.0);
    // Blender 가 그 메시에서 로우폴리 판을 만든다(창 없이 실행, 구독과 모델을 쓰지 않는다).
    let low = lowpoly::reduce(mesh, lowpoly::BUDGET, &Cancel::default()).unwrap();
    let n = low.triangle_count();
    println!("low triangles {n} / {}", lowpoly::BUDGET);
    assert!(n <= lowpoly::BUDGET && n > lowpoly::BUDGET / 2);
    assert!(low.normals.is_empty());
    for (i, frame) in &built.frames {
        let iou = model::iou(&low, &photos[*i], frame);
        println!("low {:?} IoU {iou:.3}", frame.view);
        let min = if frame.view == View::Side { 0.85 } else { 0.7 };
        assert!(iou >= min, "low {:?} IoU {iou:.3} < {min}", frame.view);
    }
    let (llo, lhi) = low.bounds();
    assert!(llo.y.abs() < 0.02, "low min y {}", llo.y);
    assert!((lhi.y - st.height_m).abs() < 0.12, "low max y {}", lhi.y);
    std::fs::write(
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("model_low.glb"),
        mytripo_public::glb::encode(&low),
    )
    .unwrap();

    // 모델 메시가 없으면 같은 입력이 부품 조립(0005)으로 간다.
    let parts = pipeline::build(&photos, &slots, &st, DEFAULT_BUDGET).unwrap();
    assert!(parts.mesh.triangle_count() <= DEFAULT_BUDGET);
    assert!(parts.mesh.normals.is_empty());

    let preview = raster::render(mesh, &raster::Camera::framing(mesh), 600, 800);
    image::RgbaImage::from_raw(600, 800, preview)
        .unwrap()
        .save(Path::new(env!("CARGO_TARGET_TMPDIR")).join("model_preview.png"))
        .unwrap();
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("model.glb");
    std::fs::write(&out, mytripo_public::glb::encode(mesh)).unwrap();
}

/// 열린 얇은 판이 많은 모델 메시(TRELLIS.2 의 SF 오토바이)도 로우폴리 판이 나온다.
/// voxel remesh 가 열린 판을 잔 조각으로 부숴서 decimate 가 예산까지 줄지 못하던 메시다(changes/019).
#[test]
fn open_sheet_model_mesh_still_gets_a_low_poly() {
    let dir = fixtures();
    let photos = sheet::load(&dir.join("moto_sf_sheet.png")).unwrap();
    let json = std::fs::read_to_string(dir.join("moto_sf_sheet_structure.json")).unwrap();
    let st = Structure::parse(&json, 3).unwrap();
    let raw = model::read_ply(&std::fs::read(dir.join("moto_sf_model.ply")).unwrap()).unwrap();
    let slots = Slots::for_photos(&st, photos.len());
    let built = pipeline::build_with(&photos, &slots, &st, model::BUDGET, Some(&raw)).unwrap();
    let low = lowpoly::reduce(&built.mesh, lowpoly::BUDGET, &Cancel::default()).unwrap();
    let n = low.triangle_count();
    println!("open sheet low triangles {n} / {}", lowpoly::BUDGET);
    assert!(n <= lowpoly::BUDGET && n > lowpoly::BUDGET / 2);
    assert_eq!(built.frames.len(), 4);
    for (i, frame) in &built.frames {
        let iou = model::iou(&low, &photos[*i], frame);
        println!("open sheet low {:?} IoU {iou:.3}", frame.view);
        let min = if frame.view == View::Side { 0.85 } else { 0.7 };
        assert!(iou >= min, "low {:?} IoU {iou:.3} < {min}", frame.view);
    }
    std::fs::write(
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("open_sheet_low.glb"),
        mytripo_public::glb::encode(&low),
    )
    .unwrap();
}

/// 열린 판 메시가 첫 시도에 예산 안으로 들어와도 형태가 부서지면 안 된다(TRELLIS.2 `512` 의 SF 오토바이).
/// 고치기 전에는 삼각형 4948 개로 끝났지만 옆 IoU 가 0.463 이었다(changes/019).
#[test]
fn open_sheet_model_mesh_within_budget_keeps_its_shape() {
    let dir = fixtures();
    let photos = sheet::load(&dir.join("moto_sf_sheet.png")).unwrap();
    let json = std::fs::read_to_string(dir.join("moto_sf_sheet_structure.json")).unwrap();
    let st = Structure::parse(&json, 3).unwrap();
    let raw = model::read_ply(&std::fs::read(dir.join("moto_sf_512_model.ply")).unwrap()).unwrap();
    let slots = Slots::for_photos(&st, photos.len());
    let built = pipeline::build_with(&photos, &slots, &st, model::BUDGET, Some(&raw)).unwrap();
    let low = lowpoly::reduce(&built.mesh, lowpoly::BUDGET, &Cancel::default()).unwrap();
    let n = low.triangle_count();
    println!("open sheet 512 low triangles {n} / {}", lowpoly::BUDGET);
    assert!(n <= lowpoly::BUDGET && n > lowpoly::BUDGET / 2);
    for (i, frame) in &built.frames {
        let iou = model::iou(&low, &photos[*i], frame);
        println!("open sheet 512 low {:?} IoU {iou:.3}", frame.view);
        let min = if frame.view == View::Side { 0.85 } else { 0.7 };
        assert!(iou >= min, "low {:?} IoU {iou:.3} < {min}", frame.view);
    }
}

/// 사람형도 로컬 모델로 만든다. 모델은 앞 칸을 본다. 로우폴리 판에 기준 마네킹의 뼈대를 넣는다(decisions/0008).
/// 모델을 돌리지 않고 저장한 모델 메시를 쓴다. Blender 는 실제로 돌린다.
#[test]
fn human_model_mesh_gets_the_mannequin_skeleton() {
    let dir = fixtures();
    let photos = sheet::load(&dir.join("man_sheet.png")).unwrap();
    let json = std::fs::read_to_string(dir.join("man_sheet_structure.json")).unwrap();
    let st = Structure::parse(&json, 3).unwrap();
    assert_eq!(st.subject, Subject::Human);
    let slots = Slots::for_photos(&st, photos.len());
    assert_eq!(model::key(&slots, st.subject), slots.front);
    let raw = model::read_ply(&std::fs::read(dir.join("man_model.ply")).unwrap()).unwrap();
    let built = pipeline::build_with(&photos, &slots, &st, model::BUDGET, Some(&raw)).unwrap();
    // 위 칸은 맞추는 데 쓰지 않는다.
    assert_eq!(built.frames.len(), 3);
    for (i, frame) in &built.frames {
        let iou = model::iou(&built.mesh, &photos[*i], frame);
        println!("man {:?} IoU {iou:.3}", frame.view);
        let min = if frame.view == View::Side { 0.7 } else { 0.9 };
        assert!(iou >= min, "{:?} IoU {iou:.3} < {min}", frame.view);
    }
    let cancel = Cancel::default();
    let low = lowpoly::reduce(&built.mesh, lowpoly::BUDGET, &cancel).unwrap();
    assert!(low.triangle_count() <= lowpoly::BUDGET);
    let joints = rig::joints(&photos, &built.frames, &st);
    assert_eq!(joints.len(), 19);
    // 인물의 왼쪽은 +X 다.
    let x = |name: &str| joints.iter().find(|j| j.0 == name).unwrap().1;
    assert!(x("shoulder_l") > 0.0 && x("shoulder_r") < 0.0);
    let glb = rig::rig(&low, &joints, &cancel).unwrap();
    // 뼈의 이름과 rest 회전이 기준 마네킹과 같다. 그래야 라이브러리의 동작 클립이 그대로 재생된다.
    let (got, want) = (rig::bones(&glb).unwrap(), rig::skeleton_bones().unwrap());
    assert_eq!(got.len(), 65);
    // 파일에 든 것은 메시 1개, 재질 1개다.
    assert_eq!(rig::counts(&glb).unwrap(), (1, 1));
    for (name, q) in &want {
        let (_, g) = got
            .iter()
            .find(|b| &b.0 == name)
            .unwrap_or_else(|| panic!("no bone {name}"));
        let dot: f32 = q.iter().zip(g).map(|(a, b)| a * b).sum();
        assert!(dot.abs() > 0.9999, "{name} rest rotation differs: {dot}");
    }
    std::fs::write(
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("man_rig.glb"),
        &glb,
    )
    .unwrap();
    // 모델 메시가 없으면 같은 입력이 로프트(0001)로 간다.
    let loft = pipeline::build(&photos, &slots, &st, DEFAULT_BUDGET).unwrap();
    assert!(!loft.rings.is_empty());
}
