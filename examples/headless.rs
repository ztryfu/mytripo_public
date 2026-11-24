//! 창 없이 앱과 같은 경로를 한 번 돌린다. 구독 CLI 를 쓴다. 검증 명령은 이 예제를 돌리지 않는다.
//!
//! cargo run --release --example headless -- <사진>...                 사진 → claude 구조 읽기 → .glb
//! cargo run --release --example headless -- --sheet-file <시트.png>    시트 → 세 칸 → claude → .glb
//! cargo run --release --example headless -- --sheet "<설명>" [참고 사진]...   codex 시트 생성 → 위와 같음
//! cargo run --release --example headless -- --edit <시트.png> "<수정 지시>"    codex 시트 수정만
//! cargo run --release --example headless -- --pose-sheet "<자세 설명>" <시트.png>   codex 자세 시트(앞·옆·3/4)만
//!
//! 덧붙이는 선택 인자(구독을 쓰지 않는 확인용):
//!   --structure <json>   claude 를 부르지 않고 저장된 구조 응답(또는 이전 작업 기록)을 쓴다
//!   --raw <ply>          로컬 모델을 돌리지 않고 저장된 모델 메시(<시각>.model.ply)를 쓴다
//!   --model-output <ply> 모델이 낸 그대로의 PLY 를 정리해서 쓴다
//!   --budget <n>         삼각형 예산
//!   --seed <n>           로컬 모델의 seed (기본 42). 가려진 쪽의 형태가 달라진다
//!   --no-model           로컬 모델을 쓰지 않는다(사람형은 로프트, 사물은 부품 조립)
//!   --model-input <png>  모델에 넣는 그림(사물은 옆 칸, 사람은 앞 칸의 배경을 지운 RGBA)을 저장한다. 다른 모델과 비교할 때 쓴다
//! 로컬 모델이 설치되어 있으면 모델로 만든다(3~10분, 불러오기 2~3분 별도). 사람형은 뼈대를 넣은 판도 낸다.

use std::path::PathBuf;

use mytripo_public::cli::Cancel;
use mytripo_public::codex::{self, SheetJob};
use mytripo_public::pipeline::{self, DEFAULT_BUDGET, Photo, Slots};
use mytripo_public::structure::{Structure, Subject};
use mytripo_public::{claude, lowpoly, model, rig, sheet};

/// 구독과 모델을 건너뛰는 선택 인자.
#[derive(Default)]
struct Opts {
    structure: Option<PathBuf>,
    raw: Option<PathBuf>,
    model_output: Option<PathBuf>,
    budget: Option<usize>,
    seed: Option<u32>,
    /// 로컬 모델을 쓰지 않는다. 사람형은 로프트, 사물은 부품 조립으로 만든다.
    no_model: bool,
    /// 모델에 넣는 그림을 저장할 경로.
    model_input: Option<PathBuf>,
}

fn build(photos: Vec<Photo>, opts: &Opts) -> Result<(), String> {
    let cancel = Cancel::default();
    // 구조 읽기는 앞·옆·뒤 세 칸만 본다. 넷째(위) 칸은 시트 위치로 정한다.
    let n = photos.len().min(3);
    let st = match &opts.structure {
        Some(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            // 이전 작업 기록이면 그 안의 구조 응답을 쓴다.
            Structure::parse(&v.get("structure").unwrap_or(&v).to_string(), n)?
        }
        None => claude::read_structure(&photos[..n], &cancel)?,
    };
    println!(
        "structure {}",
        serde_json::to_string(&st).map_err(|e| e.to_string())?
    );
    let slots = Slots::for_photos(&st, photos.len());
    let read = |p: &PathBuf| {
        model::read_ply(&std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?)
    };
    let key = model::key(&slots, st.subject);
    if let (Some(path), Some(photo)) = (&opts.model_input, key.and_then(|i| photos.get(i))) {
        model::input_image(photo)?
            .save(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        println!("model input {}", path.display());
    }
    let raw = match (&opts.raw, key) {
        _ if opts.no_model => None,
        _ if opts.model_output.is_some() => {
            Some(model::clean(read(opts.model_output.as_ref().unwrap())?)?)
        }
        (Some(p), _) => Some(read(p)?),
        (None, Some(_)) if model::available() => {
            let t = std::time::Instant::now();
            let raw = model::generate(&photos, key, opts.seed.unwrap_or(model::SEED), &cancel)?;
            println!("model {:.0}s", t.elapsed().as_secs_f32());
            Some(raw)
        }
        _ => None,
    };
    let budget = opts.budget.unwrap_or(if raw.is_some() {
        model::BUDGET
    } else {
        DEFAULT_BUDGET
    });
    let model = pipeline::build_with(&photos, &slots, &st, budget, raw.as_ref())?;
    println!("slots {slots:?}");
    println!("triangles {} / {budget}", model.mesh.triangle_count());
    for (i, frame) in &model.frames {
        let m = &photos[*i].mask;
        let sil = mytripo_public::raster::silhouette(&model.mesh, frame, m.w, m.h);
        // 모델 메시는 작은 구멍을 메운 실루엣으로 잰다(마스크와 같은 규칙).
        let iou = match &raw {
            Some(_) => model::iou(&model.mesh, &photos[*i], frame),
            None => m.iou(&sil),
        };
        println!("{:?} IoU {iou:.3}", frame.view);
        // 겹침 그림: 빨강 = 사진에만, 파랑 = 메시에만, 흰색 = 둘 다
        let overlay = image::RgbImage::from_fn(m.w as u32, m.h as u32, |x, y| {
            image::Rgb(
                match (m.get(x as i64, y as i64), sil.get(x as i64, y as i64)) {
                    (true, true) => [235, 235, 235],
                    (true, false) => [220, 40, 40],
                    (false, true) => [40, 90, 230],
                    _ => [20, 20, 20],
                },
            )
        });
        let path = std::env::temp_dir().join(format!("mytripo_public_iou_{:?}.png", frame.view));
        overlay.save(&path).map_err(|e| e.to_string())?;
        println!("overlay {}", path.display());
    }
    let saved = pipeline::save(&model.mesh)?;
    if let Some(raw) = &raw {
        let ply = saved.with_extension("model.ply");
        std::fs::write(&ply, model::write_ply(raw)).map_err(|e| e.to_string())?;
        println!("raw {}", ply.display());
    }
    let cam = mytripo_public::raster::Camera::framing(&model.mesh);
    let rgba = mytripo_public::raster::render(&model.mesh, &cam, 600, 800);
    let preview = saved.with_extension("preview.png");
    image::RgbaImage::from_raw(600, 800, rgba)
        .ok_or("preview")?
        .save(&preview)
        .map_err(|e| e.to_string())?;
    println!("saved {}", saved.display());
    println!("preview {}", preview.display());
    if raw.is_some() && lowpoly::available() {
        let low = lowpoly::reduce(&model.mesh, lowpoly::BUDGET, &cancel)?;
        let path = mytripo_public::history::low_path(&saved);
        std::fs::write(&path, mytripo_public::glb::encode(&low)).map_err(|e| e.to_string())?;
        println!("low {} triangles {}", path.display(), low.triangle_count());
        for (i, frame) in &model.frames {
            println!(
                "low {:?} IoU {:.3}",
                frame.view,
                model::iou(&low, &photos[*i], frame)
            );
        }
        let rgba = mytripo_public::raster::render(&low, &cam, 600, 800);
        let preview = saved.with_extension("low.preview.png");
        image::RgbaImage::from_raw(600, 800, rgba)
            .ok_or("preview")?
            .save(&preview)
            .map_err(|e| e.to_string())?;
        println!("low preview {}", preview.display());
        if st.subject == Subject::Human {
            let glb = rig::rig(&low, &rig::joints(&photos, &model.frames, &st), &cancel)?;
            let path = mytripo_public::history::rig_path(&saved);
            std::fs::write(&path, &glb).map_err(|e| e.to_string())?;
            println!("rig {} bones {}", path.display(), rig::bones(&glb)?.len());
        }
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut opts = Opts::default();
    let mut take = |flag: &str| {
        let k = args.iter().position(|a| a == flag)?;
        let value = args.get(k + 1).cloned();
        args.drain(k..(k + 2).min(args.len()));
        value
    };
    opts.structure = take("--structure").map(PathBuf::from);
    opts.raw = take("--raw").map(PathBuf::from);
    opts.model_output = take("--model-output").map(PathBuf::from);
    opts.budget = take("--budget").and_then(|n| n.parse().ok());
    opts.seed = take("--seed").and_then(|n| n.parse().ok());
    opts.model_input = take("--model-input").map(PathBuf::from);
    if let Some(k) = args.iter().position(|a| a == "--no-model") {
        args.remove(k);
        opts.no_model = true;
    }
    let opts = &opts;
    let cancel = Cancel::default();
    match args.first().map(String::as_str) {
        Some("--sheet-file") => {
            let path = PathBuf::from(args.get(1).ok_or("시트 경로가 필요합니다")?);
            build(sheet::load(&path)?, opts)
        }
        Some("--sheet") => {
            let job = SheetJob::New {
                description: args.get(1).cloned().unwrap_or_default(),
                refs: args[2.min(args.len())..]
                    .iter()
                    .map(PathBuf::from)
                    .collect(),
            };
            let out = codex::generate(&job, &pipeline::output_path("-sheet.png"), &cancel)?;
            println!("sheet {}", out.display());
            build(sheet::load(&out)?, opts)
        }
        Some("--pose-sheet") => {
            let job = SheetJob::Pose {
                description: args.get(1).cloned().ok_or("자세 설명이 필요합니다")?,
                sheet: PathBuf::from(args.get(2).ok_or("턴어라운드 시트 경로가 필요합니다")?),
            };
            let out = codex::generate(&job, &pipeline::output_path("-pose.png"), &cancel)?;
            println!("pose sheet {}", out.display());
            // 시점 사이 여백으로 칸을 센다. 3 이 아니면 그림을 보고 다시 만든다.
            let img = pipeline::decode(&out)?.to_rgb8();
            match sheet::split(&img) {
                Ok(panels) => println!("pose views {}", panels.len()),
                Err(e) => println!("pose views ? ({e})"),
            }
            Ok(())
        }
        Some("--edit") => {
            let job = SheetJob::Edit {
                sheet: PathBuf::from(args.get(1).ok_or("시트 경로가 필요합니다")?),
                instruction: args.get(2).cloned().ok_or("수정 지시가 필요합니다")?,
            };
            let out = codex::generate(&job, &pipeline::output_path("-sheet.png"), &cancel)?;
            println!("sheet {}", out.display());
            Ok(())
        }
        Some(_) if args.len() <= 3 => build(
            args.iter()
                .map(|p| pipeline::load_photo(p.as_ref()))
                .collect::<Result<Vec<_>, _>>()?,
            opts,
        ),
        _ => Err("사용법은 examples/headless.rs 머리 주석을 보세요".into()),
    }
}
