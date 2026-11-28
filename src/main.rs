#![windows_subsystem = "windows"]

use libmpv2::{events::Event, Mpv};

// ボタン描画用の仮想解像度(ASSオーバーレイ。実際のウィンドウサイズに自動で拡縮される)
const RES_X: f64 = 1280.0;
const RES_Y: f64 = 720.0;

// ボタンバーのレイアウト(仮想座標)
const BTN_W: f64 = 98.0;
const BTN_H: f64 = 50.0;
const GAP: f64 = 4.0;
const BAR_Y: f64 = 650.0; // バー背景の上端
const BTN_Y: f64 = 660.0; // ボタンの上端

// 1クリックあたりのズーム倍率
const STEP: f64 = 1.5;
const MAX_ZOOM_LOG2: f64 = 6.0;

// ボタン一覧(左から順)
const ACTIONS: [&str; 12] = [
    "back30", "back5", "play", "fwd5", "fwd30", "vdown", "vup", "mute", "zoom", "fit", "full",
    "filter",
];

struct State {
    // 表示モード: 0=全体表示(黒帯あり) 1=埋める(はみ出しカット) 2=引き伸ばし
    fit_mode: u8,
    zoom_mode: bool,
    filter_on: bool,
    bar_visible: bool,
    last_ui: String,
}

fn btn_x(i: usize) -> f64 {
    let total = ACTIONS.len() as f64 * (BTN_W + GAP) - GAP;
    (RES_X - total) / 2.0 + i as f64 * (BTN_W + GAP)
}

fn fmt_time(s: f64) -> String {
    let s = if s.is_finite() && s > 0.0 { s as i64 } else { 0 };
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn say(mpv: &Mpv, text: &str) {
    let _ = mpv.command("show-text", &[text, "1200"]);
}

/// 現在の状態からオーバーレイ(ボタンバー)のASSデータを作る
fn build_ui(mpv: &Mpv, st: &State) -> String {
    if !st.bar_visible {
        return String::new();
    }
    let paused = mpv.get_property::<bool>("pause").unwrap_or(false);
    let muted = mpv.get_property::<bool>("mute").unwrap_or(false);
    let vol = mpv.get_property::<f64>("volume").unwrap_or(100.0);
    let pos = mpv.get_property::<f64>("time-pos").unwrap_or(0.0);
    let dur = mpv.get_property::<f64>("duration").unwrap_or(0.0);

    let mut lines: Vec<String> = Vec::new();

    // バー背景
    lines.push(format!(
        "{{\\an7\\pos(0,{})\\bord0\\1c&H000000&\\1a&H70&\\p1}}m 0 0 l {} 0 l {} {} l 0 {}{{\\p0}}",
        BAR_Y,
        RES_X,
        RES_X,
        RES_Y - BAR_Y,
        RES_Y - BAR_Y
    ));

    // 状態表示(時間・音量)
    lines.push(format!(
        "{{\\an1\\pos(46,{})\\fs24\\bord1\\1c&HFFFFFF&}}{} / {}    音量 {:.0}%{}",
        BAR_Y + 4.0,
        fmt_time(pos),
        fmt_time(dur),
        vol,
        if muted { " (ミュート中)" } else { "" }
    ));

    for (i, a) in ACTIONS.iter().enumerate() {
        let (label, on) = match *a {
            "back30" => ("-30秒", false),
            "back5" => ("-5秒", false),
            "play" => (if paused { "再生" } else { "一時停止" }, false),
            "fwd5" => ("+5秒", false),
            "fwd30" => ("+30秒", false),
            "vdown" => ("音量-", false),
            "vup" => ("音量+", false),
            "mute" => ("ミュート", muted),
            "zoom" => (if st.zoom_mode { "ズームON" } else { "ズーム" }, st.zoom_mode),
            "fit" => (
                match st.fit_mode {
                    1 => "埋める",
                    2 => "引き伸ばし",
                    _ => "全体表示",
                },
                st.fit_mode != 0,
            ),
            "full" => ("全画面", false),
            "filter" => ("ノイズ除去", st.filter_on),
            _ => ("", false),
        };
        let color = if on {
            if *a == "mute" {
                "&H2020C0&" // 赤系
            } else {
                "&H2060E0&" // オレンジ系
            }
        } else {
            "&H555555&"
        };
        let x = btn_x(i);
        lines.push(format!(
            "{{\\an7\\pos({},{})\\bord0\\1c{}\\1a&H20&\\p1}}m 0 0 l {} 0 l {} {} l 0 {}{{\\p0}}",
            x, BTN_Y, color, BTN_W, BTN_W, BTN_H, BTN_H
        ));
        lines.push(format!(
            "{{\\an5\\pos({},{})\\fs18\\bord1\\1c&HFFFFFF&}}{}",
            x + BTN_W / 2.0,
            BTN_Y + BTN_H / 2.0,
            label
        ));
    }
    lines.join("\n")
}

/// 状態が変わったときだけオーバーレイを更新
fn refresh_ui(mpv: &Mpv, st: &mut State) {
    let ui = build_ui(mpv, st);
    if ui == st.last_ui {
        return;
    }
    if ui.is_empty() {
        let _ = mpv.command("osd-overlay", &["1", "none", ""]);
    } else {
        let _ = mpv.command(
            "osd-overlay",
            &["1", "ass-events", &ui, &RES_X.to_string(), &RES_Y.to_string()],
        );
    }
    st.last_ui = ui;
}

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
    let aspect = mpv
        .get_property::<f64>("video-params/aspect")
        .unwrap_or(16.0 / 9.0);
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

fn do_action(mpv: &Mpv, st: &mut State, a: &str) {
    match a {
        "back30" => {
            let _ = mpv.command("seek", &["-30"]);
        }
        "back5" => {
            let _ = mpv.command("seek", &["-5"]);
        }
        "fwd5" => {
            let _ = mpv.command("seek", &["5"]);
        }
        "fwd30" => {
            let _ = mpv.command("seek", &["30"]);
        }
        "play" => {
            let _ = mpv.command("cycle", &["pause"]);
        }
        "vdown" => {
            let _ = mpv.command("add", &["volume", "-10"]);
        }
        "vup" => {
            let _ = mpv.command("add", &["volume", "10"]);
        }
        "mute" => {
            let _ = mpv.command("cycle", &["mute"]);
        }
        "full" => {
            let _ = mpv.command("cycle", &["fullscreen"]);
        }
        "filter" => {
            st.filter_on = !st.filter_on;
            let _ = mpv.command("vf", &["toggle", "lavfi=[pp=ha/va/dr]"]);
            say(
                mpv,
                if st.filter_on {
                    "ノイズ除去 ON"
                } else {
                    "ノイズ除去 OFF"
                },
            );
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
            // 表示モードを変えたらズーム・移動をリセット
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
        _ => {}
    }
    refresh_ui(mpv, st);
}

fn on_left_click(mpv: &Mpv, st: &mut State) {
    let mx = mpv.get_property::<i64>("mouse-pos/x").unwrap_or(0) as f64;
    let my = mpv.get_property::<i64>("mouse-pos/y").unwrap_or(0) as f64;
    let w = mpv.get_property::<i64>("osd-width").unwrap_or(1).max(1) as f64;
    let h = mpv.get_property::<i64>("osd-height").unwrap_or(1).max(1) as f64;
    let vx = mx / w * RES_X;
    let vy = my / h * RES_Y;

    // ボタンバー上のクリック
    if st.bar_visible && vy >= BAR_Y {
        if vy >= BTN_Y && vy <= BTN_Y + BTN_H {
            for (i, a) in ACTIONS.iter().enumerate() {
                let x = btn_x(i);
                if vx >= x && vx <= x + BTN_W {
                    do_action(mpv, st, a);
                    return;
                }
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

fn main() {
    let mut mpv = Mpv::with_initializer(|init| {
        // ===== ブロックノイズ対策(最優先)=====
        init.set_property("hwdec", "no")?;
        init.set_property("vd-lavc-skiploopfilter", "none")?;
        init.set_property("vd-lavc-skipidct", "none")?;
        init.set_property("vd-lavc-skipframe", "none")?;
        init.set_property("vd-lavc-fast", "no")?;
        init.set_property("vd-lavc-threads", 0i64)?;
        // GPUドライバなし(ソフトウェア描画)でも重くならないよう、描画側は軽くする
        // (ブロックノイズはデコード側の設定で抑えているので、描画を軽くしても影響しない)
        init.set_property("deband", "no")?;
        init.set_property("scale", "bilinear")?;
        init.set_property("cscale", "bilinear")?;
        init.set_property("dscale", "bilinear")?;
        init.set_property("framedrop", "vo")?;
        // ===== 表示 =====
        // Direct3D 11 を明示。GPUドライバが無い場合はWindows標準のソフトウェア描画(WARP)を使う
        init.set_property("vo", "gpu")?;
        init.set_property("gpu-api", "d3d11")?;
        init.set_property("gpu-context", "d3d11")?;
        init.set_property("d3d11-warp", "auto")?;
        init.set_property("d3d11-flip", "no")?;
        // ===== 音量ブースト 最大200% =====
        init.set_property("volume-max", 200i64)?;
        init.set_property("volume", 100i64)?;
        // ===== 動作 =====
        init.set_property("osc", "no")?;
        init.set_property("input-default-bindings", "no")?;
        init.set_property("input-vo-keyboard", "yes")?;
        init.set_property("force-window", "yes")?;
        init.set_property("idle", "yes")?;
        init.set_property("keep-open", "yes")?;
        init.set_property("title", "BlockNoise Player")?;
        Ok(())
    })
    .expect("mpv init failed (libmpv-2.dll が exe と同じフォルダにありますか?)");

    // ===== キー割り当て(ボタンと同じ操作のショートカット)=====
    let keys: &[(&str, &str)] = &[
        ("MBTN_LEFT", "script-message zclick"),
        ("MBTN_RIGHT", "script-message zrclick"),
        ("BS", "script-message zreset"),
        ("z", "script-message act:zoom"),
        ("h", "script-message ztogglebar"),
        ("w", "script-message act:fit"),
        ("SPACE", "script-message act:play"),
        ("LEFT", "script-message act:back5"),
        ("RIGHT", "script-message act:fwd5"),
        ("SHIFT+LEFT", "script-message act:back30"),
        ("SHIFT+RIGHT", "script-message act:fwd30"),
        ("UP", "script-message act:vup"),
        ("DOWN", "script-message act:vdown"),
        ("WHEEL_UP", "script-message act:vup"),
        ("WHEEL_DOWN", "script-message act:vdown"),
        ("m", "script-message act:mute"),
        ("f", "script-message act:full"),
        ("d", "script-message act:filter"),
        ("q", "quit"),
    ];
    for (k, c) in keys {
        let _ = mpv.command("keybind", &[k, c]);
    }

    // 起動時のファイル指定 / ドラッグ&ドロップ対応
    if let Some(path) = std::env::args().nth(1) {
        let _ = mpv.command("loadfile", &[&path]);
    }

    let mut st = State {
        fit_mode: 0,
        zoom_mode: false,
        filter_on: false,
        bar_visible: true,
        last_ui: String::from("\u{0}"),
    };
    refresh_ui(&mpv, &mut st);

    loop {
        let mut msg: Option<String> = None;
        let mut quit = false;
        match mpv.event_context_mut().wait_event(0.2) {
            Some(Ok(Event::ClientMessage(a))) => msg = a.first().map(|s| s.to_string()),
            Some(Ok(Event::Shutdown)) => quit = true,
            _ => {}
        }
        if quit {
            break;
        }
        if let Some(m) = msg {
            if let Some(a) = m.strip_prefix("act:") {
                do_action(&mpv, &mut st, a);
            } else {
                match m.as_str() {
                    "zclick" => on_left_click(&mpv, &mut st),
                    "zrclick" => on_right_click(&mpv, &st),
                    "zreset" => reset_zoom(&mpv),
                    "ztogglebar" => {
                        st.bar_visible = !st.bar_visible;
                        refresh_ui(&mpv, &mut st);
                    }
                    _ => {}
                }
            }
        }
        // 再生位置・一時停止・音量などの変化を反映
        refresh_ui(&mpv, &mut st);
    }
}
