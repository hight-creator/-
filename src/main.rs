// リリースビルドではコンソール窓を出さない
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod downloader;
mod scraper;

use eframe::egui;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

/// 画面の準備（OpenGLの初期化）が済んだら true
static APP_READY: AtomicBool = AtomicBool::new(false);

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

/// OpenGLで起動できなかったとき、wgpu（DirectX。ドライバー無しでもWindows標準のソフトウェア描画で動く）で起動し直す。
/// 1プロセスで画面（イベントループ）を2回作れないため、自分自身を別プロセスで再起動する。
fn relaunch_with_wgpu() {
    log_line("OpenGLで起動できなかったため、wgpu(DirectX)で再起動します");
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("--wgpu").spawn();
    }
}

fn run(renderer: eframe::Renderer) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([760.0, 480.0]),
        renderer,
        ..Default::default()
    };
    eframe::run_native(
        "Instagram ストーリーダウンローダー",
        options,
        Box::new(|cc| {
            APP_READY.store(true, Ordering::SeqCst);
            app::setup_fonts(&cc.egui_ctx);
            Ok(Box::new(app::App::new()))
        }),
    )
}

fn main() -> eframe::Result<()> {
    let use_wgpu = std::env::args().any(|a| a == "--wgpu");

    std::panic::set_hook(Box::new(move |info| {
        log_line(&format!("PANIC: {info}"));
        // 画面の準備前に落ちた（＝OpenGL初期化の失敗など）場合だけ、wgpuで再起動
        if !use_wgpu && !APP_READY.load(Ordering::SeqCst) {
            relaunch_with_wgpu();
        }
    }));
    log_line(if use_wgpu {
        "起動開始 (wgpu)"
    } else {
        "起動開始 (OpenGL)"
    });

    if use_wgpu {
        let r = run(eframe::Renderer::Wgpu);
        match &r {
            Ok(()) => log_line("正常終了"),
            Err(e) => log_line(&format!("起動エラー(wgpu): {e}")),
        }
        return r;
    }

    match run(eframe::Renderer::Glow) {
        Ok(()) => {
            log_line("正常終了");
            Ok(())
        }
        Err(e) => {
            log_line(&format!("起動エラー(OpenGL): {e}"));
            if !APP_READY.load(Ordering::SeqCst) {
                relaunch_with_wgpu();
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}
