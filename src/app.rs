//! egui による GUI 本体

use crate::config::{self, Config, HttpMethod, ProviderConfig, Quality, ResponseFormat};
use crate::downloader;
use crate::scraper::{self, Pick, Pool, StoryItem};
use egui::{
    self, pos2, vec2, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Rect, Sense,
    TextureHandle, TextureOptions,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use tokio::sync::Semaphore;

const CARD_W: f32 = 120.0;
const CARD_H: f32 = 213.0;

/// Windows標準の日本語フォントを動的ロードして文字化けを防ぐ
pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::env::var("WINDIR")
        .map(|w| PathBuf::from(w).join("Fonts"))
        .unwrap_or_else(|_| PathBuf::from("C:\\Windows\\Fonts"));
    for name in ["meiryo.ttc", "YuGothM.ttc", "msgothic.ttc"] {
        if let Ok(bytes) = std::fs::read(dir.join(name)) {
            fonts
                .font_data
                .insert("jp".to_owned(), FontData::from_owned(bytes).into());
            // 既定フォントの後ろに追加 → 英数字は既定、日本語だけフォールバックで描画
            for fam in [FontFamily::Proportional, FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().push("jp".to_owned());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}

#[derive(PartialEq, Clone)]
enum Status {
    Idle,
    Loading,
    Done,
    Error(String),
}

struct AccountView {
    status: Status,
    items: Vec<StoryItem>,
}

enum Msg {
    Account {
        username: String,
        result: Result<Vec<StoryItem>, String>,
    },
    Thumb {
        key: String,
        tex: TextureHandle,
    },
    Preview {
        key: String,
        tex: Option<TextureHandle>,
    },
    DlProgress {
        done: usize,
        total: usize,
    },
    DlFinished {
        ok: usize,
        fail: usize,
    },
    Log(String),
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Stories,
    Settings,
}

struct PreviewState {
    item: StoryItem,
    tex: Option<TextureHandle>,
    sel: usize,
}

pub struct App {
    cfg: Config,
    rt: tokio::runtime::Runtime,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    pool: Arc<Pool>,
    views: HashMap<String, AccountView>,
    thumbs: HashMap<String, TextureHandle>,
    selected: HashSet<String>,
    tab: Tab,
    new_user: String,
    new_tag: String,
    tag_filter: String,
    proxy_text: String,
    preview: Option<PreviewState>,
    log: Vec<String>,
    dl: Option<(usize, usize)>,
    checking: usize,
    headers_text: String,
    dirty: bool,
}

impl App {
    pub fn new() -> Self {
        let cfg = Config::load();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("tokio runtime");
        let (tx, rx) = mpsc::channel();
        let proxy_text = cfg.proxies.join("\n");
        let headers_text = cfg.provider.headers.join("\n");
        let mut app = Self {
            cfg,
            rt,
            tx,
            rx,
            pool: Arc::new(Pool::new(&[])),
            views: HashMap::new(),
            thumbs: HashMap::new(),
            selected: HashSet::new(),
            tab: Tab::Stories,
            new_user: String::new(),
            new_tag: String::new(),
            tag_filter: String::new(),
            proxy_text,
            preview: None,
            log: Vec::new(),
            dl: None,
            checking: 0,
            headers_text,
            dirty: false,
        };
        app.rebuild_pool();
        app
    }

    fn rebuild_pool(&mut self) {
        let proxies: Vec<String> = if self.cfg.use_proxy {
            self.cfg.proxies.clone()
        } else {
            Vec::new()
        };
        self.pool = Arc::new(Pool::new(&proxies));
    }

    fn push_log(&mut self, s: String) {
        self.log.push(s);
        if self.log.len() > 300 {
            self.log.drain(..100);
        }
    }

    fn save(&mut self) {
        if let Err(e) = self.cfg.save() {
            self.push_log(format!("config.json の保存に失敗: {e}"));
        }
    }

    // ───────── メッセージ受信 ─────────
    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Account { username, result } => {
                    self.checking = self.checking.saturating_sub(1);
                    let line = match result {
                        Ok(items) => {
                            let n = items.len();
                            self.views.insert(
                                username.clone(),
                                AccountView {
                                    status: Status::Done,
                                    items,
                                },
                            );
                            format!("@{username}: ストーリー {n}件")
                        }
                        Err(e) => {
                            self.views.insert(
                                username.clone(),
                                AccountView {
                                    status: Status::Error(e.clone()),
                                    items: Vec::new(),
                                },
                            );
                            format!("@{username}: 失敗 - {e}")
                        }
                    };
                    self.push_log(line);
                }
                Msg::Thumb { key, tex } => {
                    self.thumbs.insert(key, tex);
                }
                Msg::Preview { key, tex } => {
                    if let Some(p) = self.preview.as_mut() {
                        if p.item.key() == key {
                            p.tex = tex;
                        }
                    }
                }
                Msg::DlProgress { done, total } => self.dl = Some((done, total)),
                Msg::DlFinished { ok, fail } => {
                    self.dl = None;
                    self.push_log(format!("ダウンロード完了: 成功 {ok} / 失敗 {fail}"));
                }
                Msg::Log(s) => self.push_log(s),
            }
        }
    }

    // ───────── 一括取得 ─────────
    fn start_check(&mut self, ctx: &egui::Context) {
        let names: Vec<String> = self.cfg.accounts.iter().map(|a| a.username.clone()).collect();
        if names.is_empty() {
            self.push_log("アカウントが未登録です".into());
            return;
        }
        self.rebuild_pool();
        self.views.clear();
        self.thumbs.clear();
        self.selected.clear();
        for n in &names {
            self.views.insert(
                n.clone(),
                AccountView {
                    status: Status::Loading,
                    items: Vec::new(),
                },
            );
        }
        self.checking = names.len();
        self.push_log(format!("{}アカウントを取得開始", names.len()));

        let sem = Arc::new(Semaphore::new(self.cfg.max_concurrency.max(1)));
        let thumb_sem = Arc::new(Semaphore::new(8));
        let retries = self.cfg.max_retries;
        let provider = Arc::new(self.cfg.provider.clone());

        for name in names {
            let tx = self.tx.clone();
            let pool = self.pool.clone();
            let ctx = ctx.clone();
            let sem = sem.clone();
            let tsem = thumb_sem.clone();
            let provider = provider.clone();
            self.rt.spawn(async move {
                let res = {
                    let _permit = sem.acquire().await.ok();
                    scraper::fetch_stories(&pool, &provider, &name, retries).await
                };
                match res {
                    Ok(items) => {
                        let _ = tx.send(Msg::Account {
                            username: name.clone(),
                            result: Ok(items.clone()),
                        });
                        ctx.request_repaint();
                        for it in items {
                            if it.thumb_url.is_empty() {
                                continue; // サムネ無し（読込中表示のまま）
                            }
                            let tx = tx.clone();
                            let pool = pool.clone();
                            let ctx = ctx.clone();
                            let tsem = tsem.clone();
                            tokio::spawn(async move {
                                let _p = tsem.acquire().await.ok();
                                let key = it.key();
                                if let Ok(bytes) =
                                    scraper::download_bytes(&pool, &it.thumb_url, retries).await
                                {
                                    if let Some(img) = decode_async(bytes, 360).await {
                                        let tex = ctx.load_texture(
                                            format!("th-{key}"),
                                            img,
                                            TextureOptions::LINEAR,
                                        );
                                        let _ = tx.send(Msg::Thumb { key, tex });
                                        ctx.request_repaint();
                                    }
                                }
                            });
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Account {
                            username: name,
                            result: Err(e.to_string()),
                        });
                        ctx.request_repaint();
                    }
                }
            });
        }
    }

    // ───────── ダウンロード ─────────
    fn visible_items(&self, only_selected: bool) -> Vec<StoryItem> {
        let mut out = Vec::new();
        for a in &self.cfg.accounts {
            if !self.tag_filter.is_empty() && a.tag != self.tag_filter {
                continue;
            }
            if let Some(v) = self.views.get(&a.username) {
                for it in &v.items {
                    if !only_selected || self.selected.contains(&it.key()) {
                        out.push(it.clone());
                    }
                }
            }
        }
        out
    }

    fn start_download(&mut self, ctx: &egui::Context, items: Vec<StoryItem>, pick: Pick) {
        if items.is_empty() {
            self.push_log("ダウンロード対象がありません".into());
            return;
        }
        if self.dl.is_some() {
            self.push_log("ダウンロード中です。完了までお待ちください".into());
            return;
        }
        let total = items.len();
        self.dl = Some((0, total));
        let dir = PathBuf::from(&self.cfg.save_dir);
        let organize = self.cfg.organize_by_user;
        let retries = self.cfg.max_retries;
        let pool = self.pool.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let sem = Arc::new(Semaphore::new(self.cfg.max_concurrency.clamp(1, 6)));

        self.rt.spawn(async move {
            let done = Arc::new(AtomicUsize::new(0));
            let fails = Arc::new(AtomicUsize::new(0));
            let mut handles = Vec::new();
            for it in items {
                let (pool, tx, ctx, sem) = (pool.clone(), tx.clone(), ctx.clone(), sem.clone());
                let (done, fails, dir) = (done.clone(), fails.clone(), dir.clone());
                handles.push(tokio::spawn(async move {
                    let _p = sem.acquire().await.ok();
                    match downloader::download_item(&pool, &it, pick, &dir, organize, retries).await
                    {
                        Ok(path) => {
                            let _ = tx.send(Msg::Log(format!("保存: {}", path.display())));
                        }
                        Err(e) => {
                            fails.fetch_add(1, Ordering::Relaxed);
                            let _ = tx.send(Msg::Log(format!("失敗 @{} {}: {e}", it.username, it.id)));
                        }
                    }
                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                    let _ = tx.send(Msg::DlProgress { done: d, total });
                    ctx.request_repaint();
                }));
            }
            for h in handles {
                let _ = h.await;
            }
            let fail = fails.load(Ordering::Relaxed);
            let _ = tx.send(Msg::DlFinished {
                ok: total - fail,
                fail,
            });
            ctx.request_repaint();
        });
    }

    // ───────── プレビュー ─────────
    fn open_preview(&mut self, ctx: &egui::Context, item: StoryItem) {
        let key = item.key();
        let url = item.image_url.clone();
        self.preview = Some(PreviewState {
            item,
            tex: None,
            sel: 0,
        });
        let (tx, pool, ctx, retries) = (
            self.tx.clone(),
            self.pool.clone(),
            ctx.clone(),
            self.cfg.max_retries,
        );
        self.rt.spawn(async move {
            let tex = match scraper::download_bytes(&pool, &url, retries).await {
                Ok(b) => decode_async(b, 1080)
                    .await
                    .map(|img| ctx.load_texture(format!("pv-{key}"), img, TextureOptions::LINEAR)),
                Err(_) => None,
            };
            let _ = tx.send(Msg::Preview { key, tex });
            ctx.request_repaint();
        });
    }

    fn preview_window(&mut self, ctx: &egui::Context) {
        let Some(p) = self.preview.as_mut() else {
            return;
        };
        let mut open = true;
        let mut save: Option<Pick> = None;
        let versions = scraper::sorted_variants(&p.item.videos);
        let title = format!("プレビュー @{}", p.item.username);
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                match &p.tex {
                    Some(tex) => {
                        let s = tex.size_vec2();
                        let scale = (460.0 / s.y).min(400.0 / s.x).min(1.0);
                        let (rect, _) = ui.allocate_exact_size(s * scale, Sense::hover());
                        ui.painter().image(
                            tex.id(),
                            rect,
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    None => {
                        ui.label("読み込み中…");
                    }
                }
                ui.label(format!(
                    "{} / {}",
                    if p.item.is_video { "動画" } else { "静止画" },
                    downloader::format_time(p.item.taken_at, "%Y-%m-%d %H:%M:%S")
                ));
                if p.item.is_video {
                    ui.label("保存する解像度を選択:");
                    for (i, v) in versions.iter().enumerate() {
                        let label = if v.width > 0 {
                            format!("{}x{}", v.width, v.height)
                        } else {
                            "元画質（解像度の選択なし）".to_string()
                        };
                        ui.radio_value(&mut p.sel, i, label);
                    }
                    if ui.button("選択した解像度で保存").clicked() {
                        if let Some(v) = versions.get(p.sel) {
                            save = Some(Pick::Exact(v.width, v.height));
                        }
                    }
                } else if ui.button("画像を保存").clicked() {
                    save = Some(Pick::Quality(Quality::Best));
                }
            });
        let item = p.item.clone();
        if !open {
            self.preview = None;
        }
        if let Some(pick) = save {
            self.start_download(ctx, vec![item], pick);
        }
    }

    // ───────── UI部品 ─────────
    fn top_panel(&mut self, ctx: &egui::Context) {
        let (mut start, mut dl_sel, mut dl_all) = (false, false, false);
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Stories, "ストーリー一覧");
                ui.selectable_value(&mut self.tab, Tab::Settings, "設定・プロキシ");
                ui.separator();
                let busy = self.checking > 0;
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new("登録アカウントをすべて一括チェック（更新）"),
                    )
                    .clicked()
                {
                    start = true;
                }
                ui.separator();

                let before = self.cfg.quality;
                egui::ComboBox::new("quality", "既定画質")
                    .selected_text(self.cfg.quality.label())
                    .show_ui(ui, |ui| {
                        for q in Quality::ALL {
                            ui.selectable_value(&mut self.cfg.quality, q, q.label());
                        }
                    });
                if before != self.cfg.quality {
                    self.dirty = true;
                }

                if ui
                    .add_enabled(
                        self.dl.is_none(),
                        egui::Button::new(format!("選択をダウンロード ({})", self.selected.len())),
                    )
                    .clicked()
                {
                    dl_sel = true;
                }
                if ui
                    .add_enabled(self.dl.is_none(), egui::Button::new("全件ダウンロード"))
                    .clicked()
                {
                    dl_all = true;
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.checkbox(&mut self.cfg.hide_empty, "0件のアカウントを非表示").changed() {
                    self.dirty = true;
                }
                let mut tags: Vec<String> = self
                    .cfg
                    .accounts
                    .iter()
                    .map(|a| a.tag.clone())
                    .filter(|t| !t.is_empty())
                    .collect();
                tags.sort();
                tags.dedup();
                let shown = if self.tag_filter.is_empty() {
                    "全タグ".to_string()
                } else {
                    self.tag_filter.clone()
                };
                egui::ComboBox::new("tagfilter", "タグで絞り込み")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.tag_filter, String::new(), "全タグ");
                        for t in &tags {
                            ui.selectable_value(&mut self.tag_filter, t.clone(), t.as_str());
                        }
                    });
                if ui.button("選択解除").clicked() {
                    self.selected.clear();
                }
            });
            ui.add_space(4.0);
        });

        let q = self.cfg.quality;
        if start {
            self.start_check(ctx);
        }
        if dl_sel {
            let items = self.visible_items(true);
            self.start_download(ctx, items, Pick::Quality(q));
        }
        if dl_all {
            let items = self.visible_items(false);
            self.start_download(ctx, items, Pick::Quality(q));
        }
    }

    fn bottom_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("log")
            .resizable(true)
            .default_height(90.0)
            .show(ctx, |ui| {
                if let Some((d, t)) = self.dl {
                    ui.add(
                        egui::ProgressBar::new(d as f32 / t.max(1) as f32)
                            .text(format!("ダウンロード {d}/{t}")),
                    );
                }
                if self.checking > 0 {
                    ui.label(format!("取得中… 残り {} アカウント", self.checking));
                }
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for l in &self.log {
                            ui.label(egui::RichText::new(l.as_str()).small());
                        }
                    });
            });
    }

    fn side_panel(&mut self, ctx: &egui::Context) {
        let mut add = false;
        let mut remove: Option<usize> = None;
        egui::SidePanel::left("side")
            .default_width(250.0)
            .show(ctx, |ui| {
                ui.heading("登録アカウント");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.new_user)
                        .hint_text("ユーザー名（@可・複数可）")
                        .desired_width(f32::INFINITY),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    add = true;
                }
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_tag)
                            .hint_text("タグ（任意）")
                            .desired_width(110.0),
                    );
                    if ui.button("追加").clicked() {
                        add = true;
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (i, a) in self.cfg.accounts.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let (txt, col) = match self.views.get(&a.username).map(|v| &v.status) {
                                Some(Status::Loading) => ("取得中", Color32::YELLOW),
                                Some(Status::Done) => ("済", Color32::LIGHT_GREEN),
                                Some(Status::Error(_)) => ("失敗", Color32::LIGHT_RED),
                                _ => ("-", Color32::GRAY),
                            };
                            ui.colored_label(col, txt);
                            ui.label(format!("@{}", a.username));
                            if !a.tag.is_empty() {
                                ui.weak(format!("[{}]", a.tag));
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("削除").clicked() {
                                        remove = Some(i);
                                    }
                                },
                            );
                        });
                    }
                });
            });

        if add {
            let n = self.cfg.add_accounts(&self.new_user, &self.new_tag);
            if n > 0 {
                self.new_user.clear();
                self.dirty = true;
            }
        }
        if let Some(i) = remove {
            let u = self.cfg.accounts.remove(i).username;
            self.views.remove(&u);
            self.dirty = true;
        }
    }

    fn stories_ui(&mut self, ui: &mut egui::Ui) -> Option<StoryItem> {
        let mut open: Option<StoryItem> = None;
        let accounts: Vec<(String, String)> = self
            .cfg
            .accounts
            .iter()
            .filter(|a| self.tag_filter.is_empty() || a.tag == self.tag_filter)
            .map(|a| (a.username.clone(), a.tag.clone()))
            .collect();
        if accounts.is_empty() {
            ui.label("左のパネルでアカウントを登録し、「一括チェック」を押してください。");
            return None;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (user, tag) in &accounts {
                    let (count, status) = match self.views.get(user) {
                        Some(v) => (v.items.len(), v.status.clone()),
                        None => (0, Status::Idle),
                    };
                    if self.cfg.hide_empty && status == Status::Done && count == 0 {
                        continue;
                    }
                    let mut title = format!("@{user}");
                    if !tag.is_empty() {
                        title.push_str(&format!("  [{tag}]"));
                    }
                    title.push_str(&format!("  ({count}件)"));
                    egui::CollapsingHeader::new(title)
                        .id_salt(user.clone())
                        .default_open(true)
                        .show(ui, |ui| match &status {
                            Status::Loading => {
                                ui.label("取得中…");
                            }
                            Status::Error(e) => {
                                ui.colored_label(Color32::LIGHT_RED, e.as_str());
                            }
                            Status::Idle => {
                                ui.weak("未取得");
                            }
                            Status::Done if count == 0 => {
                                ui.weak("公開ストーリーはありません");
                            }
                            Status::Done => {
                                if let Some(view) = self.views.get(user) {
                                    if ui.small_button("このアカウントを全選択").clicked() {
                                        for it in &view.items {
                                            self.selected.insert(it.key());
                                        }
                                    }
                                    ui.horizontal_wrapped(|ui| {
                                        for item in &view.items {
                                            let key = item.key();
                                            ui.vertical(|ui| {
                                                ui.set_width(CARD_W);
                                                let (rect, resp) = ui.allocate_exact_size(
                                                    vec2(CARD_W, CARD_H),
                                                    Sense::click(),
                                                );
                                                ui.painter().rect_filled(
                                                    rect,
                                                    4.0,
                                                    Color32::from_gray(35),
                                                );
                                                match self.thumbs.get(&key) {
                                                    Some(tex) => {
                                                        let r = fit(rect, tex.size_vec2());
                                                        ui.painter().image(
                                                            tex.id(),
                                                            r,
                                                            Rect::from_min_max(
                                                                pos2(0.0, 0.0),
                                                                pos2(1.0, 1.0),
                                                            ),
                                                            Color32::WHITE,
                                                        );
                                                    }
                                                    None => {
                                                        ui.painter().text(
                                                            rect.center(),
                                                            Align2::CENTER_CENTER,
                                                            "読込中…",
                                                            FontId::proportional(12.0),
                                                            Color32::GRAY,
                                                        );
                                                    }
                                                }
                                                let badge = Rect::from_min_size(
                                                    rect.min + vec2(4.0, 4.0),
                                                    vec2(44.0, 16.0),
                                                );
                                                ui.painter().rect_filled(
                                                    badge,
                                                    3.0,
                                                    Color32::from_black_alpha(170),
                                                );
                                                ui.painter().text(
                                                    badge.center(),
                                                    Align2::CENTER_CENTER,
                                                    if item.is_video { "動画" } else { "静止画" },
                                                    FontId::proportional(11.0),
                                                    Color32::WHITE,
                                                );
                                                if resp.clicked() {
                                                    open = Some(item.clone());
                                                }
                                                resp.on_hover_cursor(egui::CursorIcon::PointingHand);

                                                ui.small(downloader::format_time(
                                                    item.taken_at,
                                                    "%m/%d %H:%M",
                                                ));
                                                let (w, h) = item.max_resolution();
                                                if w > 0 {
                                                    ui.small(format!("最高 {w}x{h}"));
                                                } else {
                                                    ui.small("元画質");
                                                }
                                                let mut sel = self.selected.contains(&key);
                                                if ui.checkbox(&mut sel, "選択").changed() {
                                                    if sel {
                                                        self.selected.insert(key.clone());
                                                    } else {
                                                        self.selected.remove(&key);
                                                    }
                                                }
                                            });
                                        }
                                    });
                                }
                            }
                        });
                }
            });
        open
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("保存");
            ui.horizontal(|ui| {
                ui.label("保存先フォルダ:");
                if ui.text_edit_singleline(&mut self.cfg.save_dir).changed() {
                    self.dirty = true;
                }
            });
            if ui
                .checkbox(
                    &mut self.cfg.organize_by_user,
                    "アカウント別フォルダに振り分け（保存先/{username}/{username}_{timestamp}_{id}.拡張子）",
                )
                .changed()
            {
                self.dirty = true;
            }

            ui.separator();
            ui.heading("取得");
            if ui
                .add(egui::Slider::new(&mut self.cfg.max_concurrency, 1..=16).text("同時接続数"))
                .changed()
            {
                self.dirty = true;
            }
            if ui
                .add(egui::Slider::new(&mut self.cfg.max_retries, 0..=10).text("リトライ回数"))
                .changed()
            {
                self.dirty = true;
            }

            ui.separator();
            ui.heading("プロキシ（HTTP / SOCKS5）");
            if ui
                .checkbox(&mut self.cfg.use_proxy, "プロキシを使う（429/403/タイムアウト時は自動で次へ切替）")
                .changed()
            {
                self.rebuild_pool();
                self.dirty = true;
            }
            ui.label("1行1件: ip:port / user:pass@ip:port / socks5://user:pass@ip:port（#で始まる行は無視）");
            ui.add(
                egui::TextEdit::multiline(&mut self.proxy_text)
                    .desired_rows(8)
                    .desired_width(f32::INFINITY),
            );
            if ui.button("プロキシリストを保存").clicked() {
                self.cfg.proxies = self
                    .proxy_text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(String::from)
                    .collect();
                self.rebuild_pool();
                self.dirty = true;
                self.push_log(format!("プロキシ {}件を保存", self.cfg.proxies.len()));
            }

            ui.separator();
            ui.heading("取得先（Webビューア型サービスの裏側API）");
            ui.label("{username} は URL・本文・ヘッダ値の中で登録ユーザー名に置換されます。相手の仕様が変わったらここを書き換えるだけで追従できます。");
            let mut ch = false;
            let mut reset = false;
            if ui.button("Grabfy の設定に戻す").clicked() {
                reset = true;
            }
            {
                let p = &mut self.cfg.provider;
                ch |= provider_field(ui, "取得先URL", &mut p.url);
                ui.horizontal(|ui| {
                    egui::ComboBox::new("pv_method", "メソッド")
                        .selected_text(if p.method == HttpMethod::Post { "POST" } else { "GET" })
                        .show_ui(ui, |ui| {
                            ch |= ui.selectable_value(&mut p.method, HttpMethod::Post, "POST").changed();
                            ch |= ui.selectable_value(&mut p.method, HttpMethod::Get, "GET").changed();
                        });
                    egui::ComboBox::new("pv_format", "応答形式")
                        .selected_text(if p.format == ResponseFormat::Json { "JSON" } else { "HTML" })
                        .show_ui(ui, |ui| {
                            ch |= ui.selectable_value(&mut p.format, ResponseFormat::Json, "JSON").changed();
                            ch |= ui.selectable_value(&mut p.format, ResponseFormat::Html, "HTML").changed();
                        });
                });
                ch |= provider_field(ui, "Content-Type", &mut p.content_type);
                ch |= provider_field(ui, "POST本文", &mut p.body_template);
                ui.label("追加ヘッダ（1行1件: Name: value）");
                if ui
                    .add(
                        egui::TextEdit::multiline(&mut self.headers_text)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    )
                    .changed()
                {
                    p.headers = self
                        .headers_text
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(String::from)
                        .collect();
                    ch = true;
                }
                if p.format == ResponseFormat::Json {
                    ui.label("JSON抽出ルール（image_versions2 / video_versions 形式なら自動認識）");
                    ch |= provider_field(ui, "配列のパス", &mut p.items_path);
                    ch |= provider_field(ui, "IDキー", &mut p.key_id);
                    ch |= provider_field(ui, "メディアURLキー", &mut p.key_media);
                    ch |= provider_field(ui, "サムネURLキー", &mut p.key_thumb);
                    ch |= provider_field(ui, "時刻キー", &mut p.key_time);
                    ch |= provider_field(ui, "種別キー", &mut p.key_type);
                    ch |= provider_field(ui, "画質別動画の配列キー", &mut p.key_versions);
                } else {
                    ui.label("HTML抽出ルール（正規表現。名前付きグループ: url必須 / thumb type w h time id）");
                    ch |= provider_field(ui, "正規表現", &mut p.html_regex);
                }
                ch |= provider_field(ui, "非公開判定の文字列", &mut p.private_marker);
                ch |= provider_field(ui, "非公開のJSONパス(カンマ区切り)", &mut p.private_path);
                ch |= provider_field(ui, "準備中のJSONパス", &mut p.pending_path);
                ch |= provider_field(ui, "準備中のポーリング本文", &mut p.poll_body_template);
                ch |= ui
                    .add(
                        egui::Slider::new(&mut p.min_interval_ms, 0..=15000)
                            .text("最小リクエスト間隔(ms)／接続元ごと"),
                    )
                    .changed();
                ui.label("429で止められる場合は間隔を長くし、プロキシを増やしてください。429を受けた接続元は、相手の指定秒数だけ自動で休ませます。");
            }
            if reset {
                self.cfg.provider = ProviderConfig::grabfy();
                self.headers_text = self.cfg.provider.headers.join("\n");
                ch = true;
            }
            if ch {
                self.dirty = true;
            }

            ui.separator();
            ui.label(format!("設定ファイル: {}", config::config_path().display()));
        });
    }
}

impl egui_software_backend::App for App {
    fn update(&mut self, ctx: &egui::Context, _backend: &mut egui_software_backend::SoftwareBackend) {
        self.poll();
        self.top_panel(ctx);
        self.bottom_panel(ctx);
        self.side_panel(ctx);

        let mut open_item: Option<StoryItem> = None;
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Stories => open_item = self.stories_ui(ui),
            Tab::Settings => self.settings_ui(ui),
        });
        if let Some(it) = open_item {
            self.open_preview(ctx, it);
        }
        self.preview_window(ctx);

        if self.dirty {
            self.save();
            self.dirty = false;
        }
    }
}

fn provider_field(ui: &mut egui::Ui, label: &str, val: &mut String) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        changed = ui
            .add(egui::TextEdit::singleline(val).desired_width(f32::INFINITY))
            .changed();
    });
    changed
}

// ───────── 画像ユーティリティ ─────────

/// アスペクト比を保って rect の中央に収める
fn fit(rect: Rect, size: egui::Vec2) -> Rect {
    if size.x <= 0.0 || size.y <= 0.0 {
        return rect;
    }
    let s = (rect.width() / size.x).min(rect.height() / size.y);
    Rect::from_center_size(rect.center(), size * s)
}

fn decode(bytes: &[u8], max: u32) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() > max || img.height() > max {
        img.thumbnail(max, max)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        rgba.as_raw(),
    ))
}

async fn decode_async(bytes: Vec<u8>, max: u32) -> Option<egui::ColorImage> {
    tokio::task::spawn_blocking(move || decode(&bytes, max))
        .await
        .ok()
        .flatten()
}
