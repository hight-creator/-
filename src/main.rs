// リリースビルドではコンソール窓を出さない
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod downloader;
mod scraper;

use egui::vec2;
use egui_software_backend::SoftwareBackendAppConfiguration;
use std::io::Write;

/// 起動時のエラーを、exeと同じフォルダの ig-story-dl-error.log に追記する
/// （画面が出ない場合の原因調査用）
fn log_line(msg: &str) {
    let path = config::exe_dir().join("ig-story-dl-error.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let t = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{t}] {msg}");
    }
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        log_line(&format!("PANIC: {info}"));
    }));
    log_line("起動開始 (CPU描画)");

    let settings = SoftwareBackendAppConfiguration::new()
        .inner_size(Some(vec2(1100.0, 720.0)))
        .title(Some("Instagram ストーリーダウンローダー".to_string()));

    let result = egui_software_backend::run_app_with_software_backend(settings, |ctx| {
        app::setup_fonts(&ctx);
        app::App::new()
    });
    match result {
        Ok(()) => log_line("正常終了"),
        Err(e) => log_line(&format!("起動エラー: {e:?}")),
    }
}
