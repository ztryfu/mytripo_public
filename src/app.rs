//! 창: 사진 DnD 또는 턴어라운드 시트 생성 → 앞·옆·뒤 칸 → 3D 미리보기. Cmd+N 은 새 작업이다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use eframe::egui::{
    self, Color32, ColorImage, Id, Key, KeyboardShortcut, Modifiers, Rect, Sense, TextureHandle,
    TextureOptions, Vec2, pos2, vec2,
};

use crate::claude;
use crate::cli::Cancel;
use crate::codex::{self, SheetJob};
use crate::glb;
use crate::history::{self, Entry, Input};
use crate::lowpoly;
use crate::mesh::Mesh;
use crate::model;
use crate::pipeline::{self, DEFAULT_BUDGET, Photo, Slots};
use crate::raster::{self, Camera};
use crate::rig;
use crate::sheet;
use crate::structure::{Structure, Subject, View};

const THUMB: Vec2 = vec2(150.0, 190.0);
const IMAGE_EXT: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

enum Msg {
    Loaded(Arc<Vec<Photo>>),
    /// 배경이 단색이 아니다. 사진을 참고 사진으로 삼아 시트를 생성해야 한다.
    NeedSheet(String),
    Read(Structure),
    Sheet(PathBuf),
    /// 로컬 모델이 메시를 냈다. 칸 배치가 같으면 다시 쓴다.
    Raw(model::Key, Arc<model::Raw>),
    /// 모델 메시를 칸에 맞춘 결과, 그 로우폴리 판, 로우폴리 판을 만들지 못한 이유.
    Built(Box<Built>),
    Failed(String),
}

/// 모델 경로의 결과.
struct Built {
    mesh: Mesh,
    /// 로우폴리 판.
    low: Option<Mesh>,
    /// 사람형: 뼈대를 넣은 glb.
    rigged: Option<Vec<u8>>,
    /// 로우폴리 판이나 뼈대를 만들지 못한 이유.
    note: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Waiting,
    Compose,
    Generating,
    Review,
    Loading,
    Reading,
    Modeling,
    Fitting,
    Ready,
    Failed,
}

pub struct App {
    stage: Stage,
    note: Option<String>,
    /// 끌어 놓은 사진 파일. 시트 생성의 참고 사진이 된다.
    files: Vec<PathBuf>,
    description: String,
    instruction: String,
    sheet: Option<PathBuf>,
    sheet_tex: Option<TextureHandle>,
    photos: Arc<Vec<Photo>>,
    thumbs: Vec<TextureHandle>,
    slots: Slots,
    structure: Option<Structure>,
    mesh: Option<Mesh>,
    mesh_rev: u64,
    /// 모델 메시의 로우폴리 판과 그 저장 경로.
    low: Option<Mesh>,
    saved_low: Option<PathBuf>,
    /// 뼈대를 넣은 판의 저장 경로(사람형).
    saved_rig: Option<PathBuf>,
    /// 미리보기에 로우폴리 판을 보인다.
    show_low: bool,
    /// 로컬 모델이 낸 메시와 그때 모델에 넣은 사진 번호.
    raw: Option<(model::Key, Arc<model::Raw>)>,
    /// 모델의 seed. "다른 형태로" 버튼이 1씩 올린다.
    seed: u32,
    /// 지금 메시의 삼각형 예산.
    budget: usize,
    /// 이전 작업을 부품 조립 결과 그대로 연다. 모델을 돌리지 않는다.
    no_model: bool,
    /// 모델 메시를 맞춘 뒤 할 일: 저장할지, 저장하지 않으면 보일 저장 경로.
    after_fit: (bool, Option<PathBuf>),
    saved: Option<PathBuf>,
    error: Option<String>,
    rx: Option<mpsc::Receiver<Msg>>,
    cancel: Cancel,
    camera: Option<Camera>,
    preview: Option<TextureHandle>,
    preview_key: Option<(Camera, [usize; 2], u64)>,
    /// 실행 인자로 받은 시트. 첫 프레임에서 연다.
    pending: Option<PathBuf>,
    /// 지금 메시의 입력. 저장할 때 기록에 남긴다.
    input: Option<Input>,
    /// 이전 작업을 다시 여는 중: 저장된 메시 경로와 칸 배치. 구조 응답이 오면 새로 저장하지 않고 만든다.
    reopen: Option<(Option<PathBuf>, Slots)>,
    /// 이전 작업 목록. 처음 화면을 그릴 때 읽는다.
    history: Option<Vec<Entry>>,
    /// 목록 그림. 프레임마다 하나씩 읽는다. 읽지 못한 그림은 None 이다.
    history_tex: HashMap<String, Option<TextureHandle>>,
}

impl App {
    /// sheet 가 있으면 첫 화면에서 그 시트로 메시를 만든다.
    pub fn new(_cc: &eframe::CreationContext<'_>, sheet: Option<PathBuf>) -> Self {
        App {
            pending: sheet,
            ..Self::fresh()
        }
    }

    fn fresh() -> Self {
        App {
            stage: Stage::Waiting,
            note: None,
            files: Vec::new(),
            description: String::new(),
            instruction: String::new(),
            sheet: None,
            sheet_tex: None,
            photos: Arc::new(Vec::new()),
            thumbs: Vec::new(),
            slots: Slots::default(),
            structure: None,
            mesh: None,
            mesh_rev: 0,
            low: None,
            saved_low: None,
            saved_rig: None,
            show_low: false,
            raw: None,
            seed: model::SEED,
            budget: DEFAULT_BUDGET,
            no_model: false,
            after_fit: (false, None),
            saved: None,
            error: None,
            rx: None,
            cancel: Cancel::default(),
            camera: None,
            preview: None,
            preview_key: None,
            pending: None,
            input: None,
            reopen: None,
            history: None,
            history_tex: HashMap::new(),
        }
    }

    /// 앞 화면으로 돌아간다. 결과에서 시트 수정·시트 생성으로 왔으면 결과로, 그 밖에는 처음 화면으로 간다.
    fn back(&mut self) {
        match self.stage {
            Stage::Review | Stage::Compose if self.mesh.is_some() => self.stage = Stage::Ready,
            _ => self.reset(),
        }
    }

    /// 현재 작업을 끝내고 처음 화면으로 돌아간다. 돌고 있는 codex·claude 는 종료한다.
    fn reset(&mut self) {
        self.cancel.cancel();
        *self = Self::fresh();
    }

    /// 돌고 있는 작업을 끝내고 새 작업 채널을 연다.
    fn spawn(
        &mut self,
        ctx: &egui::Context,
        job: impl FnOnce(&dyn Fn(Msg), &Cancel) + Send + 'static,
    ) {
        self.cancel.cancel();
        self.cancel = Cancel::default();
        let (tx, rx) = mpsc::channel();
        let cancel = self.cancel.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let send = |m: Msg| {
                let _ = tx.send(m);
                ctx.request_repaint();
            };
            job(&send, &cancel);
        });
        self.rx = Some(rx);
        self.error = None;
    }

    fn on_drop(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let mut images: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| IMAGE_EXT.contains(&e.to_ascii_lowercase().as_str()))
            })
            .collect();
        if images.is_empty() {
            self.note = Some("PNG·JPEG·WebP 사진만 받습니다".into());
            return;
        }
        self.note = None;
        if images.len() > 3 {
            images.truncate(3);
            self.note = Some("사진은 3장까지 받습니다. 앞의 3장만 씁니다".into());
        }
        match self.stage {
            Stage::Waiting => {
                self.files = images.clone();
                self.build_from(Input::Photos { paths: images }, ctx);
            }
            Stage::Compose => self.files = images,
            _ => self.note = Some("새 작업은 Cmd+N 으로 시작합니다".into()),
        }
    }

    /// 사진이나 시트로 메시를 만든다: 자르기·마스크 → claude 구조 읽기 → 로프트.
    fn build_from(&mut self, input: Input, ctx: &egui::Context) {
        self.structure = None;
        self.mesh = None;
        self.saved = None;
        self.reopen = None;
        self.raw = None;
        self.seed = model::SEED;
        self.no_model = false;
        self.input = Some(input.clone());
        self.spawn(ctx, move |send, cancel| {
            let photos = match load_input(&input) {
                Ok(p) => Arc::new(p),
                Err(m) => return send(m),
            };
            send(Msg::Loaded(photos.clone()));
            match claude::read_structure(&photos[..photos.len().min(3)], cancel) {
                Ok(s) => send(Msg::Read(s)),
                Err(e) if !cancel.is_cancelled() => send(Msg::Failed(e)),
                Err(_) => {}
            }
        });
        self.stage = Stage::Loading;
    }

    /// 이전 작업의 입력과 설명을 지금 작업으로 가져온다.
    fn adopt(&mut self, e: &Entry) {
        self.description = e
            .record
            .as_ref()
            .map(|r| r.description.clone())
            .unwrap_or_default();
        self.sheet_tex = None;
        match &e.input {
            Input::Sheet { path } => {
                self.sheet = Some(path.clone());
                self.files.clear();
            }
            Input::Photos { paths } => {
                self.sheet = None;
                self.files = paths.clone();
            }
        }
    }

    /// 이전 작업을 저장된 구조 응답과 칸 배치로 다시 연다. claude 를 부르지 않고 새로 저장하지 않는다.
    fn open(&mut self, e: &Entry, ctx: &egui::Context) {
        let Some(rec) = e.record.clone() else { return };
        self.adopt(e);
        self.structure = None;
        self.mesh = None;
        self.saved = None;
        self.camera = None;
        self.raw = None;
        self.input = Some(e.input.clone());
        let input = e.input.clone();
        self.spawn(ctx, move |send, _| match load_input(&input) {
            Ok(p) => {
                send(Msg::Loaded(Arc::new(p)));
                send(Msg::Read(rec.structure));
            }
            Err(m) => send(m),
        });
        self.reopen = Some((e.glb.clone(), rec.slots));
        self.stage = Stage::Loading;
    }

    /// 이전 작업의 입력으로 메시를 다시 만든다. claude 가 구조를 새로 읽는다.
    fn remake(&mut self, e: &Entry, ctx: &egui::Context) {
        self.adopt(e);
        self.camera = None;
        self.build_from(e.input.clone(), ctx);
    }

    /// codex 로 시트를 만들거나 고친다.
    fn generate(&mut self, job: SheetJob, ctx: &egui::Context) {
        self.spawn(ctx, move |send, cancel| {
            let out = pipeline::output_path("-sheet.png");
            match codex::generate(&job, &out, cancel) {
                Ok(p) => send(Msg::Sheet(p)),
                Err(e) if !cancel.is_cancelled() => send(Msg::Failed(e)),
                Err(_) => {}
            }
        });
        self.stage = Stage::Generating;
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else { return };
        let msgs: Vec<Msg> = rx.try_iter().collect();
        for m in msgs {
            match m {
                Msg::Loaded(photos) => {
                    self.thumbs = photos
                        .iter()
                        .enumerate()
                        .map(|(i, p)| {
                            let t = image::imageops::thumbnail(
                                &p.rgb,
                                (THUMB.x * 2.0) as u32,
                                (THUMB.y * 2.0) as u32,
                            );
                            let img = ColorImage::from_rgb(
                                [t.width() as usize, t.height() as usize],
                                t.as_raw(),
                            );
                            ctx.load_texture(format!("photo-{i}"), img, TextureOptions::LINEAR)
                        })
                        .collect();
                    self.slots = Slots::in_order(photos.len());
                    self.photos = photos;
                    self.stage = Stage::Reading;
                }
                Msg::NeedSheet(e) => {
                    self.rx = None;
                    self.note = Some(format!(
                        "{e}. 이 사진을 참고해서 앞·옆·뒤 시트를 생성하세요"
                    ));
                    self.stage = Stage::Compose;
                }
                Msg::Read(s) => {
                    self.rx = None;
                    match self.reopen.take() {
                        Some((glb, slots)) => {
                            self.slots = slots;
                            let subject = s.subject;
                            self.structure = Some(s);
                            // 모델로 만든 작업은 저장해 둔 모델 메시를 다시 맞춘다. 없으면 모델을 돌리지 않는다.
                            self.raw = glb
                                .as_ref()
                                .and_then(|g| std::fs::read(history::model_path(g)).ok())
                                .and_then(|b| model::read_ply(&b).ok())
                                .map(|r| (model::key(&slots, subject), Arc::new(r)));
                            if let Some((_, r)) = &self.raw {
                                self.seed = r.seed;
                            }
                            self.no_model = self.raw.is_none();
                            self.make(false, glb, ctx);
                        }
                        None => {
                            self.slots = Slots::for_photos(&s, self.photos.len());
                            self.structure = Some(s);
                            self.rebuild(ctx);
                        }
                    }
                }
                Msg::Sheet(path) => {
                    self.rx = None;
                    match pipeline::decode(&path) {
                        Ok(img) => {
                            let rgba = img.to_rgba8();
                            let size = [rgba.width() as usize, rgba.height() as usize];
                            let tex = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                            self.sheet_tex =
                                Some(ctx.load_texture("sheet", tex, TextureOptions::LINEAR));
                            self.sheet = Some(path);
                            self.instruction.clear();
                            self.stage = Stage::Review;
                        }
                        Err(e) => {
                            self.error = Some(e);
                            self.stage = Stage::Failed;
                        }
                    }
                }
                Msg::Raw(key, raw) => {
                    self.raw = Some((key, raw));
                    self.stage = Stage::Fitting;
                }
                Msg::Built(built) => {
                    let Built {
                        mesh,
                        low,
                        rigged,
                        note,
                    } = *built;
                    self.rx = None;
                    if note.is_some() {
                        self.note = note;
                    }
                    let (save, shown) = std::mem::take(&mut self.after_fit);
                    self.finish(Ok(mesh), low, rigged, save, shown, true);
                }
                Msg::Failed(e) => {
                    self.error = Some(e);
                    self.stage = Stage::Failed;
                    self.rx = None;
                }
            }
        }
    }

    /// 지금 칸 배치로 메시를 만들고 바로 저장한다.
    fn rebuild(&mut self, ctx: &egui::Context) {
        self.make(true, None, ctx);
    }

    /// seed 를 바꿔 모델을 다시 돌린다. 가려진 쪽의 형태가 달라진다. claude 는 부르지 않는다.
    fn reseed(&mut self, ctx: &egui::Context) {
        self.seed += 1;
        self.raw = None;
        self.no_model = false;
        self.rebuild(ctx);
    }

    /// 지금 결과가 로컬 모델의 메시이고 모델을 다시 돌릴 수 있는지.
    fn can_reseed(&self) -> bool {
        self.stage == Stage::Ready && self.raw.is_some() && model::available()
    }

    /// 사람이 아닌 대상을 로컬 모델로 만드는지. 모델이 없어도 저장해 둔 모델 메시가 있으면 그것을 쓴다.
    fn uses_model(&self) -> bool {
        let Some(st) = &self.structure else {
            return false;
        };
        let key = model::key(&self.slots, st.subject);
        key.is_some()
            && !self.no_model
            && (model::available() || self.raw.as_ref().is_some_and(|(k, _)| *k == key))
    }

    /// 지금 칸 배치로 메시를 만든다. save 면 glb 와 이전 작업 기록을 저장하고, 아니면 shown 을 저장 경로로 보인다.
    fn make(&mut self, save: bool, shown: Option<PathBuf>, ctx: &egui::Context) {
        let Some(st) = &self.structure else { return };
        if !self.uses_model() {
            let result =
                pipeline::build(&self.photos, &self.slots, st, DEFAULT_BUDGET).map(|m| m.mesh);
            return self.finish(result, None, None, save, shown, false);
        }
        // 모델 생성(3~10분)과 맞추기(수 초)는 작업 스레드에서 한다.
        let key = model::key(&self.slots, st.subject);
        let cached = self
            .raw
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, r)| r.clone());
        let (photos, slots, st, seed) = (self.photos.clone(), self.slots, st.clone(), self.seed);
        self.after_fit = (save, shown);
        self.stage = if cached.is_some() {
            Stage::Fitting
        } else {
            Stage::Modeling
        };
        self.spawn(ctx, move |send, cancel| {
            let raw = match cached {
                Some(r) => r,
                None => match model::generate(&photos, key, seed, cancel) {
                    Ok(r) => {
                        let r = Arc::new(r);
                        send(Msg::Raw(key, r.clone()));
                        r
                    }
                    Err(e) if !cancel.is_cancelled() => return send(Msg::Failed(e)),
                    Err(_) => return,
                },
            };
            match pipeline::build_with(&photos, &slots, &st, model::BUDGET, Some(&raw)) {
                Ok(m) => {
                    // Blender 로 로우폴리 판을 만든다. 실패해도 촘촘한 메시는 낸다.
                    let (low, note) = match lowpoly::reduce(&m.mesh, lowpoly::BUDGET, cancel) {
                        Ok(l) => (Some(l), None),
                        Err(_) if cancel.is_cancelled() => return,
                        Err(e) => (None, Some(format!("로우폴리 판을 만들지 못했습니다: {e}"))),
                    };
                    // 사람형은 로우폴리 판에 기준 마네킹의 뼈대를 넣는다.
                    let (rigged, note) = match &low {
                        Some(l) if st.subject == Subject::Human => {
                            match rig::rig(l, &rig::joints(&photos, &m.frames, &st), cancel) {
                                Ok(g) => (Some(g), note),
                                Err(_) if cancel.is_cancelled() => return,
                                Err(e) => (None, Some(format!("뼈대를 넣지 못했습니다: {e}"))),
                            }
                        }
                        _ => (None, note),
                    };
                    send(Msg::Built(Box::new(Built {
                        mesh: m.mesh,
                        low,
                        rigged,
                        note,
                    })))
                }
                Err(e) => send(Msg::Failed(e)),
            }
        });
    }

    /// 만든 메시를 화면에 올린다. save 면 glb, 로우폴리 판, 이전 작업 기록, 모델 메시를 저장한다.
    fn finish(
        &mut self,
        result: Result<Mesh, String>,
        low: Option<Mesh>,
        rigged: Option<Vec<u8>>,
        save: bool,
        shown: Option<PathBuf>,
        from_model: bool,
    ) {
        let result = result.and_then(|mesh| {
            let Some(st) = self.structure.as_ref().filter(|_| save) else {
                return Ok((mesh, shown));
            };
            let path = pipeline::save(&mesh)?;
            // 모델 메시를 함께 남긴다. 이전 작업을 열 때 모델을 다시 돌리지 않는다.
            if from_model
                && let Some((_, raw)) = &self.raw
                && let Err(e) = std::fs::write(history::model_path(&path), model::write_ply(raw))
            {
                self.note = Some(format!("모델 메시를 남기지 못했습니다: {e}"));
            }
            if let Some(input) = &self.input {
                let rgba = raster::render(&mesh, &Camera::framing(&mesh), 192, 192);
                let thumb = image::RgbaImage::from_raw(192, 192, rgba).ok_or("미리보기 그림")?;
                // 기록을 남기지 못해도 메시는 저장되었다. 알리기만 한다.
                if let Err(e) = history::save(
                    &pipeline::output_dir(),
                    &path,
                    input,
                    &self.description,
                    st,
                    self.slots,
                    &thumb,
                ) {
                    self.note = Some(format!("이전 작업 기록을 남기지 못했습니다: {e}"));
                }
            }
            Ok((mesh, Some(path)))
        });
        match result {
            Ok((mesh, path)) => {
                if self.camera.is_none() {
                    self.camera = Some(Camera::framing(&mesh));
                }
                let low_path = path.as_deref().map(history::low_path);
                if save
                    && let (Some(l), Some(p)) = (&low, &low_path)
                    && let Err(e) = std::fs::write(p, glb::encode(l))
                {
                    self.note = Some(format!("로우폴리 판을 저장하지 못했습니다: {e}"));
                }
                self.saved_low = low_path.filter(|p| low.is_some() && p.exists());
                let rig_path = path.as_deref().map(history::rig_path);
                if save
                    && let (Some(g), Some(p)) = (&rigged, &rig_path)
                    && let Err(e) = std::fs::write(p, g)
                {
                    self.note = Some(format!("뼈대를 넣은 판을 저장하지 못했습니다: {e}"));
                }
                self.saved_rig = rig_path.filter(|p| p.exists());
                self.low = low;
                self.show_low = false;
                self.mesh = Some(mesh);
                self.mesh_rev += 1;
                self.saved = path;
                self.budget = if from_model {
                    model::BUDGET
                } else {
                    DEFAULT_BUDGET
                };
                self.error = None;
                self.stage = Stage::Ready;
            }
            Err(e) => {
                self.mesh = None;
                self.low = None;
                self.saved = None;
                self.saved_low = None;
                self.saved_rig = None;
                self.error = Some(e);
                self.stage = Stage::Failed;
            }
        }
    }

    fn status(&self) -> String {
        match self.stage {
            Stage::Waiting => "사진을 기다립니다".into(),
            Stage::Compose => "시트 생성 준비".into(),
            Stage::Generating => "codex 가 앞·옆·뒤 시트를 그리는 중 (1~5분)".into(),
            Stage::Review => "시트를 확인하세요".into(),
            Stage::Loading => "배경을 지우는 중".into(),
            Stage::Reading => "claude 가 앞·옆·뒤와 관절을 읽는 중".into(),
            Stage::Modeling => {
                "로컬 모델이 형태를 만드는 중 (3~10분, 첫 실행은 가중치 16 GB 를 내려받습니다)"
                    .into()
            }
            Stage::Fitting => "모델 메시를 칸에 맞추고 로우폴리 판과 뼈대를 만드는 중".into(),
            Stage::Ready => {
                let n = self.mesh.as_ref().map_or(0, Mesh::triangle_count);
                let low = self.low.as_ref().map_or(String::new(), |l| {
                    format!(" · 로우폴리 {} / {}", l.triangle_count(), lowpoly::BUDGET)
                });
                let low = if self.saved_rig.is_some() {
                    format!("{low} · 뼈 65")
                } else {
                    low
                };
                let seed = if self.raw.is_some() {
                    format!(" · seed {}", self.seed)
                } else {
                    String::new()
                };
                format!(
                    "완료 · 삼각형 {n} / {}{low}{seed} · 사진을 다른 칸으로 끌면 다시 만듭니다",
                    self.budget
                )
            }
            Stage::Failed => "실패".into(),
        }
    }

    fn busy(&self) -> bool {
        matches!(
            self.stage,
            Stage::Generating | Stage::Loading | Stage::Reading | Stage::Modeling | Stage::Fitting
        )
    }

    fn waiting_ui(&mut self, ui: &mut egui::Ui, hovering: bool) {
        let full = ui.available_rect_before_wrap();
        let drop = Rect::from_min_max(full.min, pos2(full.max.x, full.max.y - 170.0));
        let stroke_color = if hovering {
            ui.visuals().selection.stroke.color
        } else {
            ui.visuals().weak_text_color()
        };
        ui.painter().rect_stroke(
            drop.shrink(16.0),
            12.0,
            egui::Stroke::new(if hovering { 3.0 } else { 1.5 }, stroke_color),
            egui::StrokeKind::Inside,
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(drop), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(drop.height() / 2.0 - 50.0);
                ui.label(
                    egui::RichText::new("사진을 끌어 놓으세요")
                        .size(30.0)
                        .strong(),
                );
                ui.add_space(8.0);
                ui.label("1~3장 · 초록 또는 흰색 단색 배경 · 앞·옆·뒤는 AI 가 판별합니다");
                if let Some(n) = &self.note {
                    ui.add_space(8.0);
                    ui.colored_label(ui.visuals().warn_fg_color, n);
                }
            });
        });
        let rest = Rect::from_min_max(pos2(full.min.x, drop.max.y), full.max);
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(rest.shrink2(vec2(16.0, 8.0))),
            |ui| {
                ui.label("또는 만들 것을 글로 설명하면 codex 가 앞·옆·뒤·위 시트를 그립니다");
                self.compose_fields(ui);
            },
        );
    }

    /// 설명 입력과 생성 버튼. 참고 사진이 있으면 함께 보인다.
    fn compose_fields(&mut self, ui: &mut egui::Ui) {
        if !self.files.is_empty() {
            let names: Vec<String> = self.files.iter().map(|p| pipeline::file_name(p)).collect();
            ui.label(format!("참고 사진: {}", names.join(", ")));
        }
        ui.add(
            egui::TextEdit::multiline(&mut self.description)
                .hint_text("예: 배낭을 멘 여행자, 사이드백이 달린 오토바이, 육각 등불")
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        let ready = !self.description.trim().is_empty() || !self.files.is_empty();
        if ui
            .add_enabled(ready, egui::Button::new("시트 생성"))
            .clicked()
        {
            let job = SheetJob::New {
                description: self.description.clone(),
                refs: self.files.clone(),
            };
            let ctx = ui.ctx().clone();
            self.generate(job, &ctx);
        }
    }

    fn compose_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("앞·옆·뒤 시트 생성");
        if let Some(n) = &self.note {
            ui.colored_label(ui.visuals().warn_fg_color, n);
        }
        ui.label("참고 사진을 이 창에 끌어 놓으면 바꿀 수 있습니다.");
        ui.add_space(6.0);
        self.compose_fields(ui);
    }

    fn review_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(path) = self.sheet.clone() else {
            return;
        };
        // 이전 작업에서 연 시트는 그림을 아직 읽지 않았다.
        if self.sheet_tex.is_none()
            && let Ok(img) = pipeline::decode(&path)
        {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let tex = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            self.sheet_tex = Some(ctx.load_texture("sheet", tex, TextureOptions::LINEAR));
        }
        ui.horizontal(|ui| {
            if ui.button("이 시트로 진행").clicked() {
                self.build_from(Input::Sheet { path: path.clone() }, &ctx);
            }
            if ui.button("다시 생성").clicked() {
                let job = SheetJob::New {
                    description: self.description.clone(),
                    refs: self.files.clone(),
                };
                self.generate(job, &ctx);
            }
        });
        ui.horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.instruction)
                    .hint_text("수정 지시. 예: 머리를 더 작게, 망토를 빼 줘")
                    .desired_width(ui.available_width() - 70.0),
            );
            let go = ui.add_enabled(
                !self.instruction.trim().is_empty(),
                egui::Button::new("수정"),
            );
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if (go.clicked() || enter) && !self.instruction.trim().is_empty() {
                let job = SheetJob::Edit {
                    sheet: path.clone(),
                    instruction: self.instruction.clone(),
                };
                self.generate(job, &ctx);
            }
        });
        ui.add_space(6.0);
        if let Some(t) = &self.sheet_tex {
            let s = t.size_vec2();
            let avail = ui.available_size();
            let size = s * (avail.x / s.x).min(avail.y / s.y).min(1.0);
            ui.image((t.id(), size));
        }
    }

    fn slots_ui(&mut self, ui: &mut egui::Ui) {
        let can_move = self.structure.is_some() && !self.busy();
        let mut moved = None;
        ui.horizontal(|ui| {
            for v in View::ALL {
                let photo = self.slots.get(v);
                let frame = egui::Frame::default().inner_margin(8.0).corner_radius(6.0);
                let (_, dropped) = ui.dnd_drop_zone::<usize, ()>(frame, |ui| {
                    ui.set_min_size(THUMB + vec2(0.0, 40.0));
                    ui.set_max_width(THUMB.x);
                    ui.vertical_centered(|ui| {
                        ui.strong(v.label());
                        match photo.and_then(|i| self.thumbs.get(i).map(|t| (i, t))) {
                            Some((i, tex)) => {
                                let s = tex.size_vec2();
                                let size = s * (THUMB.x / s.x).min(THUMB.y / s.y);
                                if can_move {
                                    ui.dnd_drag_source(Id::unique(("photo", i)), i, |ui| {
                                        ui.image((tex.id(), size));
                                    });
                                } else {
                                    ui.image((tex.id(), size));
                                }
                                ui.small(&self.photos[i].name);
                            }
                            None => {
                                ui.add_space(THUMB.y / 2.0);
                                ui.weak("비어 있음");
                            }
                        }
                    });
                });
                if let Some(i) = dropped {
                    moved = Some((*i, v));
                }
            }
            // 시트의 위 칸: 사람이 아닌 대상을 깎을 때만 쓴다. 옮기지 않는다.
            if let Some(t) = self.slots.top.and_then(|i| self.thumbs.get(i)) {
                egui::Frame::default().inner_margin(8.0).show(ui, |ui| {
                    ui.set_min_size(THUMB + vec2(0.0, 40.0));
                    ui.set_max_width(THUMB.x);
                    ui.vertical_centered(|ui| {
                        ui.strong(View::Top.label());
                        let s = t.size_vec2();
                        ui.image((t.id(), s * (THUMB.x / s.x).min(THUMB.y / s.y)));
                    });
                });
            }
            if self.stage == Stage::Ready || self.stage == Stage::Failed {
                ui.vertical(|ui| {
                    if self.sheet.is_some() {
                        if ui.button("시트 수정").clicked() {
                            self.stage = Stage::Review;
                        }
                    } else if !self.files.is_empty()
                        && ui.button("이 사진을 참고해서 시트 생성").clicked()
                    {
                        self.stage = Stage::Compose;
                    }
                    if self.can_reseed()
                        && ui
                            .button("다른 형태로 다시 만들기")
                            .on_hover_text(
                                "모델의 seed 를 바꿔 다시 돌립니다 (3~10분, claude 호출 없음)",
                            )
                            .clicked()
                    {
                        let ctx = ui.ctx().clone();
                        self.reseed(&ctx);
                    }
                    if self.stage == Stage::Ready && self.low.is_some() {
                        let label = if self.show_low {
                            "촘촘한 메시 보기"
                        } else {
                            "로우폴리 보기"
                        };
                        if ui.button(label).clicked() {
                            self.show_low = !self.show_low;
                            self.mesh_rev += 1;
                        }
                    }
                });
            }
        });
        if let Some((i, v)) = moved
            && self.slots.move_photo(i, v)
        {
            let ctx = ui.ctx().clone();
            self.rebuild(&ctx);
        }
    }

    /// 처음 화면 오른쪽의 이전 작업 목록. 열기는 저장된 구조로, 다시 만들기는 claude 를 새로 불러 만든다.
    fn history_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let entries = self
            .history
            .get_or_insert_with(|| history::list(&pipeline::output_dir()))
            .clone();
        if entries.is_empty() {
            return;
        }
        let mut action: Option<(bool, Entry)> = None;
        let mut decoded = false;
        egui::Panel::right("history")
            .resizable(false)
            .exact_size(300.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.heading("이전 작업");
                ui.weak("열기: 저장된 결과 · 다시 만들기: claude 가 새로 읽습니다");
                ui.add_space(4.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for e in &entries {
                        // 그림은 한 프레임에 하나만 읽는다. 시트 원본은 커서 오래 걸린다.
                        if !self.history_tex.contains_key(&e.stamp) && !decoded {
                            decoded = true;
                            let tex = image::open(&e.thumb).ok().map(|img| {
                                let t = img.thumbnail(128, 128).to_rgba8();
                                let size = [t.width() as usize, t.height() as usize];
                                ctx.load_texture(
                                    format!("history-{}", e.stamp),
                                    ColorImage::from_rgba_unmultiplied(size, t.as_raw()),
                                    TextureOptions::LINEAR,
                                )
                            });
                            self.history_tex.insert(e.stamp.clone(), tex);
                        }
                        ui.horizontal(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
                            ui.painter().rect_filled(rect, 4.0, Color32::from_gray(40));
                            if let Some(Some(t)) = self.history_tex.get(&e.stamp) {
                                let s = t.size_vec2();
                                let fit = s * (64.0 / s.x).min(64.0 / s.y);
                                ui.painter().image(
                                    t.id(),
                                    Rect::from_center_size(rect.center(), fit),
                                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                    Color32::WHITE,
                                );
                            }
                            ui.vertical(|ui| {
                                ui.add(egui::Label::new(e.title()).truncate());
                                ui.weak(e.time());
                                ui.horizontal(|ui| {
                                    if e.record.is_some() && ui.small_button("열기").clicked() {
                                        action = Some((true, e.clone()));
                                    }
                                    if ui.small_button("다시 만들기").clicked() {
                                        action = Some((false, e.clone()));
                                    }
                                });
                            });
                        });
                        ui.add_space(4.0);
                    }
                });
            });
        if decoded {
            ctx.request_repaint();
        }
        match action {
            Some((true, e)) => self.open(&e, &ctx),
            Some((false, e)) => self.remake(&e, &ctx),
            None => {}
        }
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui) {
        let shown = match &self.low {
            Some(low) if self.show_low => Some(low),
            _ => self.mesh.as_ref(),
        };
        let (Some(mesh), Some(cam)) = (shown, &mut self.camera) else {
            return;
        };
        let size = ui.available_size().max(vec2(64.0, 64.0));
        let (rect, resp) = ui.allocate_exact_size(size, Sense::drag());
        if resp.dragged() {
            let d = resp.drag_delta();
            cam.yaw -= d.x * 0.01;
            cam.pitch = (cam.pitch + d.y * 0.01).clamp(-1.45, 1.45);
        }
        if resp.hovered() {
            let s = ui.input(|i| i.smooth_scroll_delta().y);
            if s != 0.0 {
                cam.distance = (cam.distance * (-s * 0.003).exp()).clamp(0.05, 100.0);
            }
        }
        let ppp = ui.ctx().pixels_per_point();
        let w = ((rect.width() * ppp) as usize).clamp(16, 1600);
        let h = ((rect.height() * ppp) as usize).clamp(16, 1600);
        let key = (*cam, [w, h], self.mesh_rev);
        if self.preview_key != Some(key) {
            let rgba = raster::render(mesh, cam, w, h);
            let img = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
            match &mut self.preview {
                Some(t) => t.set(img, TextureOptions::LINEAR),
                None => {
                    self.preview = Some(ui.ctx().load_texture(
                        "preview",
                        img,
                        TextureOptions::LINEAR,
                    ))
                }
            }
            self.preview_key = Some(key);
        }
        if let Some(t) = &self.preview {
            ui.painter().image(
                t.id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        ui.painter().text(
            rect.left_bottom() + vec2(10.0, -10.0),
            egui::Align2::LEFT_BOTTOM,
            "드래그: 회전 · 휠: 확대",
            egui::FontId::proportional(13.0),
            Color32::from_gray(170),
        );
    }
}

/// 입력을 칸 사진으로 읽는다. 배경이 단색이 아닌 사진은 시트 생성을 권하는 메시지가 된다.
fn load_input(input: &Input) -> Result<Vec<Photo>, Msg> {
    match input {
        Input::Photos { paths } => {
            let mut out = Vec::new();
            for p in paths {
                let img = pipeline::decode(p).map_err(Msg::Failed)?;
                out.push(
                    pipeline::photo_from_image(pipeline::file_name(p), img)
                        .map_err(Msg::NeedSheet)?,
                );
            }
            Ok(out)
        }
        Input::Sheet { path } => sheet::load(path).map_err(Msg::Failed),
    }
}

fn path_label(ui: &mut egui::Ui, label: &str, p: &Path) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::Label::new(egui::RichText::new(p.display().to_string()).monospace())
                .selectable(true),
        );
    });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::N)))
        {
            self.reset();
        }
        if let Some(p) = self.pending.take() {
            self.sheet = Some(p.clone());
            self.build_from(Input::Sheet { path: p }, &ctx);
        }
        self.poll(&ctx);
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.on_drop(dropped, &ctx);
        }
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());

        egui::Panel::top("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if self.stage != Stage::Waiting && ui.button("← 뒤로").clicked() {
                    self.back();
                }
                ui.heading("mytripo_public");
                ui.separator();
                if self.busy() {
                    ui.spinner();
                }
                ui.label(self.status());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak("Cmd+N 새 작업");
                });
            });
        });
        egui::Panel::bottom("output").show(ui, |ui| {
            ui.add_space(4.0);
            if let Some(p) = &self.sheet {
                path_label(ui, "시트:", p);
            }
            if let Some(p) = &self.saved {
                path_label(ui, "저장:", p);
            }
            if let Some(p) = &self.saved_low {
                path_label(ui, "로우폴리:", p);
            }
            if let Some(p) = &self.saved_rig {
                path_label(ui, "뼈대:", p);
            }
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
                if self.stage == Stage::Failed
                    && (self.sheet.is_some() || !self.description.is_empty())
                {
                    ui.horizontal(|ui| {
                        if self.sheet.is_some() && ui.button("시트 확인으로").clicked() {
                            self.stage = Stage::Review;
                        }
                        if ui.button("시트 생성으로").clicked() {
                            self.stage = Stage::Compose;
                        }
                    });
                }
            }
            if !matches!(self.stage, Stage::Waiting | Stage::Compose)
                && let Some(n) = &self.note
            {
                ui.colored_label(ui.visuals().warn_fg_color, n);
            }
            ui.add_space(4.0);
        });
        if self.stage == Stage::Waiting {
            self.history_ui(ui);
        }
        egui::CentralPanel::default().show(ui, |ui| match self.stage {
            Stage::Waiting => self.waiting_ui(ui, hovering),
            Stage::Compose => self.compose_ui(ui),
            Stage::Review => self.review_ui(ui),
            Stage::Generating => {
                ui.centered_and_justified(|ui| {
                    ui.label("codex 가 시트를 그리고 있습니다. Cmd+N 으로 취소합니다.");
                });
            }
            _ if self.photos.is_empty() => {}
            _ => {
                self.slots_ui(ui);
                ui.separator();
                self.preview_ui(ui);
            }
        });
    }
}
