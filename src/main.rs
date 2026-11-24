fn main() -> eframe::Result {
    // cargo run --release -- --sheet-file <시트.png>: 시트로 바로 시작한다.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sheet = match args.as_slice() {
        [flag, path] if flag == "--sheet-file" => Some(std::path::PathBuf::from(path)),
        _ => None,
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("mytripo_public")
            .with_inner_size([1100.0, 820.0])
            .with_drag_and_drop(true)
            // 넣지 않으면 eframe 이 실행 중에 Dock 아이콘을 egui 로고로 바꾼다.
            .with_icon(icon()),
        ..Default::default()
    };
    eframe::run_native(
        "mytripo_public",
        options,
        Box::new(|cc| Ok(Box::new(mytripo_public::app::App::new(cc, sheet)))),
    )
}

/// 앱 아이콘. `scripts/make_app.sh` 가 같은 PNG 로 번들의 .icns 를 만든다.
fn icon() -> eframe::egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
        .expect("assets/icon.png 는 PNG 다")
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_is_square_1024() {
        let icon = super::icon();
        assert_eq!((icon.width, icon.height), (1024, 1024));
    }
}
