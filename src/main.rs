#![windows_subsystem = "windows"]

use libmpv2::{events::Event, Mpv};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// ============================================================
//  レイアウト(ASSオーバーレイの仮想座標 1280x720。実際のウィンドウに自動で拡縮される)
// ============================================================
const RES_X: f64 = 1280.0;
const RES_Y: f64 = 720.0;
const BAR_Y: f64 = 572.0; // 操作バー背景の上端
const INFO_Y: f64 = 596.0; // 情報テキストの下端
// シークバー
const SEEK_X: f64 = 16.0;
const SEEK_W: f64 = 1248.0;
const SEEK_Y: f64 = 602.0;
const SEEK_H: f64 = 14.0;
// 1段目ボタン
const R1_Y: f64 = 622.0;
const R1_H: f64 = 46.0;
const BTN_W: f64 = 94.0;
const GAP: f64 = 4.0;
// 2段目(速度)
const R2_Y: f64 = 674.0;
const R2_H: f64 = 40.0;
const SP_W: f64 = 84.0;
const SL_X: f64 = 288.0;
const SL_W: f64 = 980.0;

// 1クリックあたりのズーム倍率
const STEP: f64 = 1.5;
const MAX_ZOOM_LOG2: f64 = 6.0;

const ROW1: [&str; 13] = [
    "back30", "back5", "play", "fwd5", "fwd30", "vdown", "vup", "mute", "zoom", "fit", "rot",
    "full", "filter",
];

/// libmpv2 クレートが引数を自動で "..." に囲むかどうか(起動時に判定)
static CRATE_QUOTES: AtomicBool = AtomicBool::new(false);

struct State {
    fit_mode: u8, // 0=全体表示 1=埋める 2=引き伸ばし
    zoom_mode: bool,
    filter_on: bool,
    bar_manual: bool,  // h キーでの表示/非表示
    bar_visible: bool, // 実際に表示するか(自動非表示を含む)
    last_ui: String,
    last_move: Instant,
    last_mouse: (i64, i64),
    mail_last: String,
    speeds: Vec<u32>,
}

// ============================================================
//  エラー表示
// ============================================================
#[link(name = "user32")]
extern "system" {
    fn MessageBoxW(
        hwnd: *mut core::ffi::c_void,
        text: *const u16,
        caption: *const u16,
        utype: u32,
    ) -> i32;
}

/// エラーを画面に表示する(windows_subsystem=windows だと何も出ず原因が分からなくなるため)
fn msgbox(text: &str) {
    let t: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let c: Vec<u16> = "BlockNoise Player".encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), 0x10);
    }
}

// ============================================================
//  mpv コマンド送信(引数にスペース・記号があっても壊れないようにする)
// ============================================================
fn esc(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\r' => {}
            _ => o.push(c),
        }
    }
    o
}

fn cmd(mpv: &Mpv, name: &str, args: &[&str]) {
    let q = CRATE_QUOTES.load(Ordering::Relaxed);
    let enc: Vec<String> = args
        .iter()
        .map(|a| if q { esc(a) } else { format!("\"{}\"", esc(a)) })
        .collect();
    let refs: Vec<&str> = enc.iter().map(|s| s.as_str()).collect();
    let _ = mpv.command(name, &refs);
}

fn say(mpv: &Mpv, text: &str) {
    cmd(mpv, "show-text", &[text, "1200"]);
}

// ============================================================
//  表示用の補助
// ============================================================
fn fmt_time(s: f64) -> String {
    let s = if s.is_finite() && s > 0.0 { s as i64 } else { 0 };
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn speed_list() -> Vec<u32> {
    // 1〜100% は 1% 刻み、そのあと 200%,300%,...,1000%
    let mut v: Vec<u32> = (1..=100).collect();
    v.extend((2..=10).map(|k| k * 100));
    v
}

fn nearest_idx(list: &[u32], speed: f64) -> usize {
    let pct = speed * 100.0;
    let mut best = 0usize;
    let mut bd = f64::MAX;
    for (i, v) in list.iter().enumerate() {
        let d = (*v as f64 - pct).abs();
        if d < bd {
            bd = d;
            best = i;
        }
    }
    best
}

fn r1_x(i: usize) -> f64 {
    let total = ROW1.len() as f64 * (BTN_W + GAP) - GAP;
    (RES_X - total) / 2.0 + i as f64 * (BTN_W + GAP)
}

fn rect(x: f64, y: f64, w: f64, h: f64, color: &str, alpha: &str) -> String {
    format!(
        "{{\\an7\\pos({},{})\\bord0\\1c{}\\1a{}\\p1}}m 0 0 l {} 0 l {} {} l 0 {}{{\\p0}}",
        x, y, color, alpha, w, w, h, h
    )
}

fn label(cx: f64, cy: f64, size: u32, text: &str) -> String {
    format!(
        "{{\\an5\\pos({},{})\\fs{}\\bord1\\1c&HFFFFFF&}}{}",
        cx, cy, size, text
    )
}

const C_OFF: &str = "&H555555&";
const C_ON: &str = "&H2060E0&";

/// 現在の状態から操作バーのASSデータを作る
fn build_ui(mpv: &Mpv, st: &State) -> String {
    if !st.bar_visible {
        return String::new();
    }
    let paused = mpv.get_property::<bool>("pause").unwrap_or(false);
    let muted = mpv.get_property::<bool>("mute").unwrap_or(false);
    let vol = mpv.get_property::<f64>("volume").unwrap_or(100.0);
    let pos = mpv.get_property::<f64>("time-pos").unwrap_or(0.0);
    let dur = mpv.get_property::<f64>("duration").unwrap_or(0.0);
    let speed = mpv.get_property::<f64>("speed").unwrap_or(1.0);
    let rot = mpv.get_property::<i64>("video-rotate").unwrap_or(0);

    let mut l: Vec<String> = Vec::new();

    // バー背景
    l.push(rect(0.0, BAR_Y, RES_X, RES_Y - BAR_Y, "&H000000&", "&H60&"));

    // 情報テキスト
    l.push(format!(
        "{{\\an1\\pos(16,{})\\fs22\\bord1\\1c&HFFFFFF&}}{} / {}    音量 {:.0}%{}    速度 {:.0}%    回転 {}°",
        INFO_Y,
        fmt_time(pos),
        fmt_time(dur),
        vol,
        if muted { "(ミュート中)" } else { "" },
        speed * 100.0,
        rot
    ));

    // シークバー
    let frac = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) } else { 0.0 };
    l.push(rect(SEEK_X, SEEK_Y, SEEK_W, SEEK_H, "&H404040&", "&H20&"));
    l.push(rect(SEEK_X, SEEK_Y, SEEK_W * frac, SEEK_H, C_ON, "&H00&"));

    // 1段目ボタン
    for (i, a) in ROW1.iter().enumerate() {
        let (text, on): (String, bool) = match *a {
            "back30" => ("-30秒".to_string(), false),
            "back5" => ("-5秒".to_string(), false),
            "play" => (if paused { "再生" } else { "一時停止" }.to_string(), false),
            "fwd5" => ("+5秒".to_string(), false),
            "fwd30" => ("+30秒".to_string(), false),
            "vdown" => ("音量-".to_string(), false),
            "vup" => ("音量+".to_string(), false),
            "mute" => ("ミュート".to_string(), muted),
            "zoom" => (
                if st.zoom_mode { "ズームON" } else { "ズーム" }.to_string(),
                st.zoom_mode,
            ),
            "fit" => (
                match st.fit_mode {
                    1 => "埋める",
                    2 => "引き伸ばし",
                    _ => "全体表示",
                }
                .to_string(),
                st.fit_mode != 0,
            ),
            "rot" => (
                if rot != 0 {
                    format!("回転{}°", rot)
                } else {
                    "回転".to_string()
                },
                rot != 0,
            ),
            "full" => ("全画面".to_string(), false),
            "filter" => ("ノイズ除去".to_string(), st.filter_on),
            _ => (String::new(), false),
        };
        let x = r1_x(i);
        l.push(rect(x, R1_Y, BTN_W, R1_H, if on { C_ON } else { C_OFF }, "&H20&"));
        l.push(label(x + BTN_W / 2.0, R1_Y + R1_H / 2.0, 16, &text));
    }

    // 2段目: 速度 遅く / 速く / 等倍 / スライダー
    let sp_btns = [("遅く", 12.0), ("速く", 100.0), ("等倍", 188.0)];
    for (text, x) in sp_btns.iter() {
        l.push(rect(*x, R2_Y, SP_W, R2_H, C_OFF, "&H20&"));
        l.push(label(*x + SP_W / 2.0, R2_Y + R2_H / 2.0, 18, text));
    }
    let idx = nearest_idx(&st.speeds, speed);
    let n = st.speeds.len().max(2) as f64;
    let tx = SL_X + SL_W * (idx as f64) / (n - 1.0);
    l.push(rect(SL_X, R2_Y, SL_W, R2_H, "&H404040&", "&H20&"));
    l.push(rect(SL_X, R2_Y, tx - SL_X, R2_H, "&H803010&", "&H20&"));
    l.push(rect((tx - 4.0).max(SL_X), R2_Y, 8.0, R2_H, C_ON, "&H00&"));
    l.push(label(
        SL_X + SL_W / 2.0,
        R2_Y + R2_H / 2.0,
        18,
        &format!(
            "再生速度 {}%(1〜100%は1%刻み / 200〜1000%は100%刻み)",
            st.speeds[idx]
        ),
    ));

    l.join("\n")
}

/// 状態が変わったときだけオーバーレイを更新
fn refresh_ui(mpv: &Mpv, st: &mut State) {
    let ui = build_ui(mpv, st);
    if ui == st.last_ui {
        return;
    }
    if ui.is_empty() {
        cmd(mpv, "osd-overlay", &["1", "none", ""]);
    } else {
        cmd(
            mpv,
            "osd-overlay",
            &["1", "ass-events", &ui, "1280", "720"],
        );
    }
    st.last_ui = ui;
}

// ============================================================
//  ズーム
// ============================================================
fn reset_zoom(mpv: &Mpv) {
    let _ = mpv.set_property("video-zoom", 0.0f64);
    let _ = mpv.set_property("video-pan-x", 0.0f64);
    let _ = mpv.set_property("video-pan-y", 0.0f64);
    say(mpv, "ズーム: 100%");
}

/// クリック位置を動かさないようにズームする(k>1 拡大 / k<1 縮小)
fn zoom_at_cursor(mpv: &Mpv, k: f64, fit_mode: u8) {
    let mx = mpv.get_property::<i64>("mouse-pos/x").unwrap_or(0) as f64;
    let my = mpv.get_property::<i64>("mouse-pos/y").unwrap_or(0) as f64;
    let w = mpv.get_property::<i64>("osd-width").unwrap_or(1) as f64;
    let h = mpv.get_property::<i64>("osd-height").unwrap_or(1) as f64;
    let mut aspect = mpv
        .get_property::<f64>("video-params/aspect")
        .unwrap_or(16.0 / 9.0);
    let rot = mpv.get_property::<i64>("video-rotate").unwrap_or(0);
    if rot % 180 != 0 && aspect > 0.0 {
        aspect = 1.0 / aspect; // 90/270度回転中は縦横が入れ替わる
    }
    if w < 1.0 || h < 1.0 || aspect <= 0.0 {
        return;
    }
    let z = mpv.get_property::<f64>("video-zoom").unwrap_or(0.0);
    let px = mpv.get_property::<f64>("video-pan-x").unwrap_or(0.0);
    let py = mpv.get_property::<f64>("video-pan-y").unwrap_or(0.0);

    let new_z = (z + k.log2()).clamp(0.0, MAX_ZOOM_LOG2);
    let real_k = 2f64.powf(new_z - z);
    if (real_k - 1.0).abs() < 1e-9 {
        return;
    }
    if new_z <= 0.0 {
        reset_zoom(mpv);
        return;
    }

    // 表示モードごとの、ズーム前の動画表示サイズ
    let (base_w, base_h) = match fit_mode {
        1 => {
            let bw = w.max(h * aspect);
            (bw, bw / aspect)
        }
        2 => (w, h),
        _ => {
            let bw = w.min(h * aspect);
            (bw, bw / aspect)
        }
    };
    let ws = base_w * 2f64.powf(z);
    let hs = base_h * 2f64.powf(z);
    let dx = mx - w / 2.0;
    let dy = my - h / 2.0;
    let new_px = px + dx / ws * (1.0 / real_k - 1.0);
    let new_py = py + dy / hs * (1.0 / real_k - 1.0);

    let _ = mpv.set_property("video-zoom", new_z);
    let _ = mpv.set_property("video-pan-x", new_px);
    let _ = mpv.set_property("video-pan-y", new_py);
    say(mpv, &format!("ズーム: {:.0}%", 2f64.powf(new_z) * 100.0));
}

// ============================================================
//  操作
// ============================================================
fn set_speed_idx(mpv: &Mpv, st: &State, idx: usize) {
    let idx = idx.min(st.speeds.len() - 1);
    let pct = st.speeds[idx];
    let _ = mpv.set_property("speed", pct as f64 / 100.0);
    say(mpv, &format!("再生速度: {}%", pct));
}

fn do_action(mpv: &Mpv, st: &mut State, a: &str) {
    match a {
        "back30" => cmd(mpv, "seek", &["-30"]),
        "back5" => cmd(mpv, "seek", &["-5"]),
        "fwd5" => cmd(mpv, "seek", &["5"]),
        "fwd30" => cmd(mpv, "seek", &["30"]),
        "play" => {
            let p = mpv.get_property::<bool>("pause").unwrap_or(false);
            let _ = mpv.set_property("pause", !p);
        }
        "vdown" | "vup" => {
            let v = mpv.get_property::<f64>("volume").unwrap_or(100.0);
            let nv = if a == "vup" { (v + 10.0).min(200.0) } else { (v - 10.0).max(0.0) };
            let _ = mpv.set_property("volume", nv);
            say(mpv, &format!("音量: {:.0}%", nv));
        }
        "mute" => {
            let m = mpv.get_property::<bool>("mute").unwrap_or(false);
            let _ = mpv.set_property("mute", !m);
        }
        "full" => {
            let f = mpv.get_property::<bool>("fullscreen").unwrap_or(false);
            let _ = mpv.set_property("fullscreen", !f);
        }
        "filter" => {
            let want = !st.filter_on;
            let r = mpv.set_property("vf", if want { "lavfi=[pp=ha/va/dr]" } else { "" });
            if r.is_ok() {
                st.filter_on = want;
                say(mpv, if want { "ノイズ除去 ON" } else { "ノイズ除去 OFF" });
            } else {
                say(mpv, "ノイズ除去はこの環境では使えません");
            }
        }
        "rot" => {
            let cur = mpv.get_property::<i64>("video-rotate").unwrap_or(0);
            let nr = (cur + 90) % 360;
            let _ = mpv.set_property("video-rotate", nr);
            let _ = mpv.set_property("video-zoom", 0.0f64);
            let _ = mpv.set_property("video-pan-x", 0.0f64);
            let _ = mpv.set_property("video-pan-y", 0.0f64);
            say(mpv, &format!("回転: {}°", nr));
        }
        "fit" => {
            st.fit_mode = (st.fit_mode + 1) % 3;
            let (panscan, keep, name) = match st.fit_mode {
                1 => (1.0f64, "yes", "埋める(はみ出し部分はカット)"),
                2 => (0.0f64, "no", "引き伸ばし(縦横比を無視)"),
                _ => (0.0f64, "yes", "全体表示(ウィンドウに収める)"),
            };
            let _ = mpv.set_property("panscan", panscan);
            let _ = mpv.set_property("keepaspect", keep);
            let _ = mpv.set_property("video-zoom", 0.0f64);
            let _ = mpv.set_property("video-pan-x", 0.0f64);
            let _ = mpv.set_property("video-pan-y", 0.0f64);
            say(mpv, name);
        }
        "zoom" => {
            st.zoom_mode = !st.zoom_mode;
            say(
                mpv,
                if st.zoom_mode {
                    "ズームモード: クリックで拡大 / 右クリックで縮小"
                } else {
                    "ズームモード OFF"
                },
            );
        }
        "slower" | "faster" => {
            let sp = mpv.get_property::<f64>("speed").unwrap_or(1.0);
            let i = nearest_idx(&st.speeds, sp);
            let ni = if a == "faster" { i + 1 } else { i.saturating_sub(1) };
            set_speed_idx(mpv, st, ni);
        }
        "spdreset" => set_speed_idx(mpv, st, 99), // 100%
        "zreset" => reset_zoom(mpv),
        "togglebar" => st.bar_manual = !st.bar_manual,
        "quit" => cmd(mpv, "quit", &[]),
        _ => {}
    }
    refresh_ui(mpv, st);
}

fn mouse_virtual(mpv: &Mpv) -> (f64, f64) {
    let mx = mpv.get_property::<i64>("mouse-pos/x").unwrap_or(0) as f64;
    let my = mpv.get_property::<i64>("mouse-pos/y").unwrap_or(0) as f64;
    let w = mpv.get_property::<i64>("osd-width").unwrap_or(1).max(1) as f64;
    let h = mpv.get_property::<i64>("osd-height").unwrap_or(1).max(1) as f64;
    (mx / w * RES_X, my / h * RES_Y)
}

fn on_left_click(mpv: &Mpv, st: &mut State) {
    let (vx, vy) = mouse_virtual(mpv);

    if st.bar_visible && vy >= BAR_Y {
        // シークバー
        if vy >= SEEK_Y - 6.0 && vy <= SEEK_Y + SEEK_H + 4.0 && vx >= SEEK_X && vx <= SEEK_X + SEEK_W {
            let pct = ((vx - SEEK_X) / SEEK_W * 100.0).clamp(0.0, 100.0);
            let _ = mpv.set_property("percent-pos", pct);
            refresh_ui(mpv, st);
            return;
        }
        // 1段目ボタン
        if vy >= R1_Y && vy <= R1_Y + R1_H {
            for (i, a) in ROW1.iter().enumerate() {
                let x = r1_x(i);
                if vx >= x && vx <= x + BTN_W {
                    do_action(mpv, st, a);
                    return;
                }
            }
            return;
        }
        // 2段目(速度)
        if vy >= R2_Y && vy <= R2_Y + R2_H {
            if vx >= 12.0 && vx <= 12.0 + SP_W {
                do_action(mpv, st, "slower");
            } else if vx >= 100.0 && vx <= 100.0 + SP_W {
                do_action(mpv, st, "faster");
            } else if vx >= 188.0 && vx <= 188.0 + SP_W {
                do_action(mpv, st, "spdreset");
            } else if vx >= SL_X && vx <= SL_X + SL_W {
                let n = st.speeds.len();
                let f = ((vx - SL_X) / SL_W).clamp(0.0, 1.0);
                let idx = (f * (n as f64 - 1.0)).round() as usize;
                set_speed_idx(mpv, st, idx);
                refresh_ui(mpv, st);
            }
        }
        return; // バーの余白クリックは何もしない
    }

    // 動画上のクリック: ズームモードONなら拡大
    if st.zoom_mode {
        zoom_at_cursor(mpv, STEP, st.fit_mode);
    }
}

fn on_right_click(mpv: &Mpv, st: &State) {
    if st.zoom_mode {
        zoom_at_cursor(mpv, 1.0 / STEP, st.fit_mode);
    }
}

// ============================================================
//  CI用の自己診断
// ============================================================
/// ウィンドウなしで libmpv が読み込め、デコードが最後まで動くかを確認する
fn selftest() -> i32 {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default();
    let out = dir.join("selftest.txt");
    let mut mpv = match Mpv::with_initializer(|init| {
        let _ = init.set_property("vo", "null");
        let _ = init.set_property("ao", "null");
        let _ = init.set_property("idle", "yes");
        Ok(())
    }) {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::write(&out, format!("NG init: {:?}", e));
            return 2;
        }
    };
    let ver = mpv.get_property::<String>("mpv-version").unwrap_or_default();
    let _ = mpv.command(
        "loadfile",
        &["av://lavfi:testsrc=size=320x240:rate=25:duration=1"],
    );
    let start = Instant::now();
    let mut done = false;
    while start.elapsed().as_secs() < 25 {
        if let Some(Ok(Event::EndFile(_))) = mpv.event_context_mut().wait_event(0.5) {
            done = true;
            break;
        }
    }
    let _ = std::fs::write(
        &out,
        format!("{} version={}", if done { "OK" } else { "NG timeout" }, ver),
    );
    if done {
        0
    } else {
        1
    }
}

// ============================================================
//  main
// ============================================================
fn main() {
    std::panic::set_hook(Box::new(|info| {
        msgbox(&format!("エラーが発生しました:\n{}", info));
    }));

    if std::env::var("BNP_SELFTEST").is_ok() {
        std::process::exit(selftest());
    }

    let mut mpv = match Mpv::with_initializer(|init| {
        // 個々のオプションは失敗しても起動を止めない(バージョン差で未対応の名前があっても動かすため)
        macro_rules! opt {
            ($k:expr, $v:expr) => {
                let _ = init.set_property($k, $v);
            };
        }
        // ===== ブロックノイズ対策(最優先)=====
        opt!("hwdec", "no");
        opt!("vd-lavc-skiploopfilter", "none");
        opt!("vd-lavc-skipidct", "none");
        opt!("vd-lavc-skipframe", "none");
        opt!("vd-lavc-fast", "no");
        opt!("vd-lavc-threads", 0i64);
        // GPUドライバなし(ソフトウェア描画)でも重くならないよう、描画側は軽くする
        // (ブロックノイズはデコード側の設定で抑えているので、描画を軽くしても影響しない)
        opt!("deband", "no");
        opt!("scale", "bilinear");
        opt!("cscale", "bilinear");
        opt!("dscale", "bilinear");
        opt!("framedrop", "vo");
        // ===== 表示 =====
        // Direct3D 11 を明示。GPUドライバが無い場合はWindows標準のソフトウェア描画(WARP)を使う
        opt!("vo", "gpu");
        opt!("gpu-api", "d3d11");
        opt!("gpu-context", "d3d11");
        opt!("d3d11-warp", "auto");
        opt!("d3d11-flip", "no");
        // ボタンの日本語表示用フォント
        opt!("osd-font", "Meiryo");
        // ===== 音量ブースト 最大200% =====
        opt!("volume-max", 200i64);
        opt!("volume", 100i64);
        // ===== 動作 =====
        opt!("osc", "no");
        opt!("input-default-bindings", "no");
        opt!("input-vo-keyboard", "yes");
        opt!("force-window", "yes");
        opt!("idle", "yes");
        opt!("keep-open", "yes");
        opt!("title", "BlockNoise Player");
        Ok(())
    }) {
        Ok(m) => m,
        Err(e) => {
            msgbox(&format!(
                "プレイヤーを初期化できませんでした。\nlibmpv-2.dll などが exe と同じフォルダにあるか確認してください。\n\n{:?}",
                e
            ));
            std::process::exit(1);
        }
    };

    // ----- ライブラリが引数を "..." で囲むかどうかを判定 -----
    // 囲まない版なら、スペースを含む引数を渡すとエラーになる
    let crate_quotes = {
        let ok = mpv.command("set", &["force-media-title", "x y"]).is_ok();
        let v = mpv
            .get_property::<String>("force-media-title")
            .unwrap_or_default();
        let _ = mpv.set_property("force-media-title", "");
        ok && v == "x y"
    };
    CRATE_QUOTES.store(crate_quotes, Ordering::Relaxed);

    // ----- キー・マウス割り当て -----
    // 押されたら force-media-title の値が変わる → メインループで検出して処理する
    // (script-message の受信に頼らない確実な方式)
    let keys: &[(&str, &str)] = &[
        ("MBTN_LEFT", "mbl"),
        ("MBTN_RIGHT", "mbr"),
        ("BS", "zreset"),
        ("z", "zoom"),
        ("h", "togglebar"),
        ("w", "fit"),
        ("r", "rot"),
        ("SPACE", "play"),
        ("LEFT", "back5"),
        ("RIGHT", "fwd5"),
        ("SHIFT+LEFT", "back30"),
        ("SHIFT+RIGHT", "fwd30"),
        ("UP", "vup"),
        ("DOWN", "vdown"),
        ("WHEEL_UP", "vup"),
        ("WHEEL_DOWN", "vdown"),
        ("m", "mute"),
        ("f", "full"),
        ("d", "filter"),
        ("[", "slower"),
        ("]", "faster"),
        ("0", "spdreset"),
        ("q", "quit"),
    ];
    for (k, act) in keys {
        let c = format!("cycle-values force-media-title {}:a {}:b", act, act);
        cmd(&mpv, "keybind", &[k, &c]);
    }

    // 起動時のファイル指定 / ドラッグ&ドロップ対応
    if let Some(path) = std::env::args().nth(1) {
        cmd(&mpv, "loadfile", &[&path]);
    }

    let mut st = State {
        fit_mode: 0,
        zoom_mode: false,
        filter_on: false,
        bar_manual: true,
        bar_visible: true,
        last_ui: String::from("\u{0}"),
        last_move: Instant::now(),
        last_mouse: (-1, -1),
        mail_last: String::new(),
        speeds: speed_list(),
    };
    refresh_ui(&mpv, &mut st);

    loop {
        let mut quit = false;
        if let Some(Ok(Event::Shutdown)) = mpv.event_context_mut().wait_event(0.03) {
            quit = true;
        }
        if quit {
            break;
        }

        // キー・マウス操作の検出
        let v = mpv
            .get_property::<String>("force-media-title")
            .unwrap_or_default();
        if v != st.mail_last {
            st.mail_last = v.clone();
            if let Some(act) = v.split(':').next() {
                match act {
                    "mbl" => on_left_click(&mpv, &mut st),
                    "mbr" => on_right_click(&mpv, &st),
                    "" => {}
                    other => do_action(&mpv, &mut st, other),
                }
            }
        }

        // マウスが動いたらバーを表示、止まって数秒たったら隠す(一時停止中は出しっぱなし)
        let mp = (
            mpv.get_property::<i64>("mouse-pos/x").unwrap_or(-1),
            mpv.get_property::<i64>("mouse-pos/y").unwrap_or(-1),
        );
        if mp != st.last_mouse {
            st.last_mouse = mp;
            st.last_move = Instant::now();
        }
        let paused = mpv.get_property::<bool>("pause").unwrap_or(false);
        let near_bar = mouse_virtual(&mpv).1 >= BAR_Y;
        st.bar_visible = st.bar_manual
            && (paused || near_bar || st.last_move.elapsed() < Duration::from_secs(4));

        // 再生位置・音量などの変化を反映
        refresh_ui(&mpv, &mut st);
    }
}
