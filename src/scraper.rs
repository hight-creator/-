//! ストーリー取得・解析ロジック（Webビューア型サービスの裏側APIを reqwest で叩く方式）。
//!
//! 相手サービスの仕様（URL・送信内容・応答の抽出ルール）は `config::ProviderConfig` に分離してあり、
//! 仕様変更時は config.json / 設定タブを書き換えるだけで追従できる。
//! それでも足りない場合の修正箇所はこのファイルの `try_fetch`（リクエスト組み立て）と
//! `parse_json` / `parse_html`（抽出）だけ。

use crate::config::{HttpMethod, ProviderConfig, Quality, ResponseFormat};
use base64::Engine;
use regex::Regex;
use reqwest::Client;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const PROXY_COOLDOWN: Duration = Duration::from_secs(60);

// ───────── データ型 ─────────

#[derive(Clone, Debug)]
pub struct MediaVariant {
    pub width: u32,
    pub height: u32,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct StoryItem {
    pub id: String,
    pub username: String,
    pub is_video: bool,
    pub taken_at: i64,
    /// 一覧用の小さい画像（空なら読込中表示のまま）
    pub thumb_url: String,
    /// 静止画の本体URL（動画の場合はカバー画像）
    pub image_url: String,
    pub image_size: (u32, u32),
    /// 動画の解像度別URL（video_versions）
    pub videos: Vec<MediaVariant>,
}

impl StoryItem {
    pub fn key(&self) -> String {
        format!("{}:{}", self.username, self.id)
    }

    /// 利用可能な最高解像度（解像度情報がなければ 0x0）
    pub fn max_resolution(&self) -> (u32, u32) {
        if self.is_video {
            sorted_variants(&self.videos)
                .first()
                .map(|v| (v.width, v.height))
                .unwrap_or((0, 0))
        } else {
            self.image_size
        }
    }
}

/// 画質の指定方法
#[derive(Clone, Copy, Debug)]
pub enum Pick {
    Quality(Quality),
    Exact(u32, u32),
}

#[derive(Debug, Clone)]
pub enum FetchError {
    Config(String),
    Network(String),
    Status(u16),
    /// 429。値は相手が指定した待機秒数（本文の retryAfter → Retry-After → 60）
    RateLimited(u64),
    /// 相手サービスが返したエラーメッセージ（4xxの本文の error）
    Server(String),
    Private,
    NotFound,
    Parse(String),
}

impl FetchError {
    /// プロキシを替えて再試行する価値があるか
    pub fn retryable(&self) -> bool {
        match self {
            FetchError::Network(_) | FetchError::RateLimited(_) => true,
            FetchError::Status(s) => matches!(*s, 401 | 403 | 408 | 429) || *s >= 500,
            _ => false,
        }
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Config(m) => write!(f, "設定エラー: {m}"),
            FetchError::Network(e) => write!(f, "通信エラー: {e}"),
            FetchError::Status(s) => write!(f, "HTTP {s}"),
            FetchError::RateLimited(s) => write!(f, "リクエスト制限 (429)：{s}秒待機が必要です"),
            FetchError::Server(m) => write!(f, "サービスのエラー: {m}"),
            FetchError::Private => write!(f, "非公開アカウントです"),
            FetchError::NotFound => write!(f, "アカウントが見つかりません (404)"),
            FetchError::Parse(m) => write!(f, "解析エラー: {m}"),
        }
    }
}

// ───────── 画質選択 ─────────

/// 解像度の大きい順に並べ、同一解像度の重複を除く（解像度不明 0x0 は URL で区別）
pub fn sorted_variants(v: &[MediaVariant]) -> Vec<MediaVariant> {
    let mut s: Vec<MediaVariant> = v.to_vec();
    s.sort_by_key(|x| std::cmp::Reverse(x.width as u64 * x.height as u64));
    let mut seen = HashSet::new();
    s.retain(|x| {
        if x.width == 0 && x.height == 0 {
            seen.insert(x.url.clone())
        } else {
            seen.insert(format!("{}x{}", x.width, x.height))
        }
    });
    s
}

/// video_versions から指定画質のものを選ぶ。
/// Best=最大 / Low=最小 / Medium=短辺が720pxに最も近いもの / Exact=解像度一致（なければ最大）
pub fn pick_video(versions: &[MediaVariant], pick: Pick) -> Option<MediaVariant> {
    let s = sorted_variants(versions);
    if s.is_empty() {
        return None;
    }
    match pick {
        Pick::Quality(Quality::Best) => s.first().cloned(),
        Pick::Quality(Quality::Low) => s.last().cloned(),
        Pick::Quality(Quality::Medium) => s
            .iter()
            .min_by_key(|v| (v.width.min(v.height) as i64 - 720).abs())
            .cloned(),
        Pick::Exact(w, h) => s
            .iter()
            .find(|v| v.width == w && v.height == h)
            .cloned()
            .or_else(|| s.first().cloned()),
    }
}

// ───────── 共通ヘルパ ─────────

fn as_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn as_i64_loose(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.trim().parse().ok())
}

fn as_u32_loose(v: &Value) -> Option<u32> {
    as_i64_loose(v).and_then(|n| u32::try_from(n).ok())
}

fn looks_video(url: &str) -> bool {
    let path = url.split('?').next().unwrap_or(url).to_lowercase();
    path.contains(".mp4") || path.ends_with(".mov") || path.ends_with(".webm")
}

/// 中継URL（`?d=<base64url のJSON {"u":元URL}>`）なら元URLを取り出す。
/// 署名や期限が変わってもIDが変わらないようにするため。
fn proxied_target(url: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    let d = q.split('&').find_map(|kv| kv.strip_prefix("d="))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(d.trim_end_matches('='))
        .ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("u")?.as_str().map(String::from)
}

/// URLのパス末尾（拡張子なし）をIDにする。取れなければURLのハッシュ。
fn url_id(url: &str) -> String {
    let target = proxied_target(url);
    let url = target.as_deref().unwrap_or(url);
    let path = url.split('?').next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or("");
    let stem = last.rsplit_once('.').map(|(s, _)| s).unwrap_or(last);
    let id: String = stem.chars().take(60).collect();
    if id.is_empty() {
        let mut h = DefaultHasher::new();
        url.hash(&mut h);
        format!("{:x}", h.finish())
    } else {
        id
    }
}

fn unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("\\u0026", "&")
        .replace("\\/", "/")
}

// ───────── JSON解析 ─────────

fn parse_variants(v: Option<&Value>) -> Vec<MediaVariant> {
    v.and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    let url = e.get("url")?.as_str()?.to_string();
                    Some(MediaVariant {
                        width: e.get("width").and_then(as_u32_loose).unwrap_or(0),
                        height: e.get("height").and_then(as_u32_loose).unwrap_or(0),
                        url,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Instagram本体と同じ構造（image_versions2 / video_versions）の要素。
/// ビューア型サービスがInstagramのJSONをそのまま返す場合に自動で効く。
fn parse_item_ig(v: &Value, username: &str) -> Option<StoryItem> {
    let id = v.get("pk").and_then(as_string).or_else(|| {
        v.get("id")
            .and_then(as_string)
            .map(|s| s.split('_').next().unwrap_or("").to_string())
    })?;
    let taken_at = v.get("taken_at").and_then(as_i64_loose).unwrap_or(0);
    let cands = parse_variants(v.pointer("/image_versions2/candidates"));
    let videos = parse_variants(v.get("video_versions"));
    let is_video = !videos.is_empty() || v.get("media_type").and_then(Value::as_i64) == Some(2);
    let best = cands
        .iter()
        .max_by_key(|c| c.width as u64 * c.height as u64)?;
    let thumb = cands
        .iter()
        .min_by_key(|c| (c.width as i64 - 320).abs())
        .unwrap_or(best);
    Some(StoryItem {
        id,
        username: username.to_string(),
        is_video,
        taken_at,
        thumb_url: thumb.url.clone(),
        image_url: best.url.clone(),
        image_size: (best.width, best.height),
        videos,
    })
}

/// 設定のキー名で対応付ける要素（Webビューア型の独自JSON）
fn parse_item_mapped(v: &Value, p: &ProviderConfig, username: &str) -> Option<StoryItem> {
    let media = v.get(&p.key_media).and_then(Value::as_str).map(unescape)?;
    let kind = v
        .get(&p.key_type)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    let mut videos = parse_variants(v.get(&p.key_versions));
    for m in videos.iter_mut() {
        m.url = unescape(&m.url);
    }
    let is_video = !videos.is_empty() || kind.contains("video") || looks_video(&media);
    if is_video && videos.is_empty() {
        videos.push(MediaVariant {
            width: 0,
            height: 0,
            url: media.clone(),
        });
    }
    let thumb = v
        .get(&p.key_thumb)
        .and_then(Value::as_str)
        .map(unescape)
        .unwrap_or_else(|| if is_video { String::new() } else { media.clone() });
    let image_url = if is_video { thumb.clone() } else { media.clone() };
    Some(StoryItem {
        id: v
            .get(&p.key_id)
            .and_then(as_string)
            .unwrap_or_else(|| url_id(&media)),
        username: username.to_string(),
        is_video,
        taken_at: v.get(&p.key_time).and_then(as_i64_loose).unwrap_or(0),
        thumb_url: thumb,
        image_url,
        image_size: (0, 0),
        videos,
    })
}

pub fn parse_json(root: &Value, p: &ProviderConfig, username: &str) -> Vec<StoryItem> {
    let node = if p.items_path.trim().is_empty() {
        Some(root)
    } else {
        root.pointer(p.items_path.trim())
    };
    let mut out: Vec<StoryItem> = node
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| {
                    parse_item_ig(v, username).or_else(|| parse_item_mapped(v, p, username))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|i| i.taken_at);
    out
}

// ───────── HTML解析（正規表現） ─────────

pub fn parse_html(body: &str, p: &ProviderConfig, username: &str) -> Result<Vec<StoryItem>, FetchError> {
    let re = Regex::new(&p.html_regex)
        .map_err(|e| FetchError::Config(format!("html_regex が不正です: {e}")))?;
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, StoryItem> = HashMap::new();

    for c in re.captures_iter(body) {
        let Some(m) = c.name("url") else { continue };
        let url = unescape(m.as_str());
        let grp = |n: &str| c.name(n).map(|m| unescape(m.as_str()));
        let kind = grp("type").unwrap_or_default().to_lowercase();
        let is_video = kind.contains("video") || looks_video(&url);
        let w = grp("w").and_then(|s| s.trim().parse().ok()).unwrap_or(0u32);
        let h = grp("h").and_then(|s| s.trim().parse().ok()).unwrap_or(0u32);
        let ts = grp("time").and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(0);
        let id = grp("id").unwrap_or_else(|| url_id(&url));
        let variant = MediaVariant {
            width: w,
            height: h,
            url: url.clone(),
        };

        if let Some(item) = groups.get_mut(&id) {
            // 同じ id → 画質違いの動画としてまとめる
            if is_video && !item.videos.iter().any(|v| v.url == url) {
                item.videos.push(variant);
            }
            continue;
        }
        let thumb = grp("thumb").unwrap_or_else(|| if is_video { String::new() } else { url.clone() });
        order.push(id.clone());
        groups.insert(
            id.clone(),
            StoryItem {
                id,
                username: username.to_string(),
                is_video,
                taken_at: ts,
                image_url: if is_video { thumb.clone() } else { url.clone() },
                thumb_url: thumb,
                image_size: (w, h),
                videos: if is_video { vec![variant] } else { Vec::new() },
            },
        );
    }

    let mut out: Vec<StoryItem> = order.into_iter().filter_map(|id| groups.remove(&id)).collect();
    out.sort_by_key(|i| i.taken_at); // 同時刻は元の並びを保つ（安定ソート）
    Ok(out)
}

// ───────── プロキシプール ─────────

pub fn normalize_proxy(line: &str) -> Option<String> {
    let l = line.trim();
    if l.is_empty() || l.starts_with('#') {
        return None;
    }
    if l.contains("://") {
        Some(l.to_string())
    } else {
        Some(format!("http://{l}"))
    }
}

pub struct Pool {
    proxies: Vec<String>,
    cursor: AtomicUsize,
    cooldown: Mutex<HashMap<String, Instant>>,
    /// 接続元ごとの「次にリクエストしてよい時刻」
    next_slot: Mutex<HashMap<String, Instant>>,
    clients: Mutex<HashMap<String, Client>>,
}

impl Pool {
    pub fn new(raw: &[String]) -> Self {
        Self {
            proxies: raw.iter().filter_map(|l| normalize_proxy(l)).collect(),
            cursor: AtomicUsize::new(0),
            cooldown: Mutex::new(HashMap::new()),
            next_slot: Mutex::new(HashMap::new()),
            clients: Mutex::new(HashMap::new()),
        }
    }

    /// 指定時間、その接続元を使わない（直接接続は空文字キー）
    pub fn penalize_for(&self, key: &str, d: Duration) {
        self.cooldown
            .lock()
            .unwrap()
            .insert(key.to_string(), Instant::now() + d);
    }

    /// 接続元ごとにリクエスト間隔を空ける（少しゆらぎを付ける）。
    /// 429後のクールダウン中はその終了まで待つ（最大5分）。
    pub async fn throttle(&self, proxy: Option<&str>, min_ms: u64) {
        let key = proxy.unwrap_or("").to_string();
        let wait = {
            let now = Instant::now();
            let cd = self.cooldown.lock().unwrap().get(&key).copied();
            let mut slots = self.next_slot.lock().unwrap();
            let mut at = slots.get(&key).copied().unwrap_or(now).max(now);
            if let Some(c) = cd {
                at = at.max(c);
            }
            let jitter = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u64 % (min_ms / 4 + 1))
                .unwrap_or(0);
            slots.insert(key, at + Duration::from_millis(min_ms + jitter));
            at.saturating_duration_since(now)
        };
        let wait = wait.min(Duration::from_secs(300));
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// 次に使うプロキシ（ラウンドロビン、クールダウン中は飛ばす）。未設定なら None（直接接続）。
    pub fn pick(&self) -> Option<String> {
        let n = self.proxies.len();
        if n == 0 {
            return None;
        }
        let now = Instant::now();
        let cd = self.cooldown.lock().unwrap();
        for _ in 0..n {
            let i = self.cursor.fetch_add(1, Ordering::Relaxed) % n;
            let p = &self.proxies[i];
            if cd.get(p).map_or(true, |t| *t <= now) {
                return Some(p.clone());
            }
        }
        Some(self.proxies[self.cursor.fetch_add(1, Ordering::Relaxed) % n].clone())
    }

    pub fn penalize(&self, proxy: &str) {
        self.cooldown
            .lock()
            .unwrap()
            .insert(proxy.to_string(), Instant::now() + PROXY_COOLDOWN);
    }

    pub fn client(&self, proxy: Option<&str>) -> Result<Client, FetchError> {
        let key = proxy.unwrap_or("").to_string();
        let mut map = self.clients.lock().unwrap();
        if let Some(c) = map.get(&key) {
            return Ok(c.clone());
        }
        let mut b = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .gzip(true);
        if let Some(p) = proxy {
            let px = reqwest::Proxy::all(p)
                .map_err(|e| FetchError::Network(format!("プロキシ形式エラー: {e}")))?;
            b = b.proxy(px);
        }
        let c = b.build().map_err(|e| FetchError::Network(e.to_string()))?;
        map.insert(key, c.clone());
        Ok(c)
    }
}

// ───────── 通信 ─────────

/// 1回のリクエスト送信（設定どおりに組み立てる）。成功時は応答本文を返す。
async fn send(
    client: &Client,
    p: &ProviderConfig,
    username: &str,
    body_template: &str,
) -> Result<String, FetchError> {
    let url = p.url.trim().replace("{username}", username);
    let mut req = match p.method {
        HttpMethod::Post => client.post(&url),
        HttpMethod::Get => client.get(&url),
    };
    if p.method == HttpMethod::Post {
        req = req
            .header("content-type", p.content_type.as_str())
            .body(body_template.replace("{username}", username));
    }
    for h in &p.headers {
        if let Some((k, v)) = h.split_once(':') {
            req = req.header(k.trim(), v.trim().replace("{username}", username));
        }
    }

    let resp = req
        .send()
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let retry_after_header = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok());
    let text = resp
        .text()
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;
    let json: Option<Value> = serde_json::from_str(&text).ok();

    match status {
        200..=299 => Ok(text),
        429 => {
            // 本文の retryAfter（秒）を優先
            let body_wait = json
                .as_ref()
                .and_then(|v| v.get("retryAfter"))
                .and_then(as_i64_loose)
                .and_then(|n| u64::try_from(n).ok());
            Err(FetchError::RateLimited(
                body_wait.or(retry_after_header).unwrap_or(60),
            ))
        }
        404 => Err(FetchError::NotFound),
        401 | 403 | 408 => Err(FetchError::Status(status)),
        400..=499 => {
            let msg = json
                .as_ref()
                .and_then(|v| v.get("error").or_else(|| v.get("message")))
                .and_then(Value::as_str)
                .map(String::from);
            Err(match msg {
                Some(m) => FetchError::Server(m),
                None => FetchError::Status(status),
            })
        }
        s => Err(FetchError::Status(s)),
    }
}

fn is_private(v: &Value, p: &ProviderConfig) -> bool {
    p.private_path
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|ptr| v.pointer(ptr).and_then(Value::as_bool) == Some(true))
}

/// 応答本文 → (ストーリー一覧, 準備中か)
fn parse_response(
    text: &str,
    p: &ProviderConfig,
    username: &str,
) -> Result<(Vec<StoryItem>, bool), FetchError> {
    match p.format {
        ResponseFormat::Json => {
            let v: Value = serde_json::from_str(text).map_err(|_| {
                FetchError::Parse(
                    "JSONとして読めませんでした（応答がHTMLなら形式を「HTML」に切り替えてください）".into(),
                )
            })?;
            if is_private(&v, p) {
                return Err(FetchError::Private);
            }
            let pending = !p.pending_path.trim().is_empty()
                && v.pointer(p.pending_path.trim()).and_then(Value::as_bool) == Some(true);
            Ok((parse_json(&v, p, username), pending))
        }
        ResponseFormat::Html => Ok((parse_html(text, p, username)?, false)),
    }
}

/// ポーリング間隔（ミリ秒）。相手サイト自身のJSと同じ段階的な間隔。
const POLL_DELAYS_MS: [u64; 12] = [1000, 1500, 2000, 2000, 2500, 3000, 4000, 5000, 5000, 6000, 8000, 8000];

/// 1回分の取得：リクエスト → 解析 →（準備中なら）ポーリング
async fn try_fetch(
    client: &Client,
    p: &ProviderConfig,
    username: &str,
) -> Result<Vec<StoryItem>, FetchError> {
    if p.url.trim().is_empty() {
        return Err(FetchError::Config(
            "取得先URLが未設定です（設定タブの「取得先」で入力してください）".into(),
        ));
    }
    let mut text = send(client, p, username, &p.body_template).await?;
    let (mut items, mut pending) = parse_response(&text, p, username)?;

    if items.is_empty() && pending && !p.poll_body_template.trim().is_empty() {
        for d in POLL_DELAYS_MS.iter().take(p.poll_max as usize) {
            tokio::time::sleep(Duration::from_millis(*d)).await;
            match send(client, p, username, &p.poll_body_template).await {
                Ok(t) => text = t,
                // 429は呼び出し側で接続元を休ませるので中断。それ以外の一時エラーは次の回へ
                Err(e @ FetchError::RateLimited(_)) => return Err(e),
                Err(e) if e.retryable() => continue,
                Err(e) => return Err(e),
            }
            let (it, pe) = parse_response(&text, p, username)?;
            items = it;
            pending = pe;
            if !items.is_empty() || !pending {
                break;
            }
        }
    }

    if items.is_empty()
        && !p.private_marker.trim().is_empty()
        && text.to_lowercase().contains(&p.private_marker.trim().to_lowercase())
    {
        return Err(FetchError::Private);
    }
    Ok(items)
}

/// 1アカウント分のストーリー取得。429/403/タイムアウト等では別プロキシでリトライする。
pub async fn fetch_stories(
    pool: &Pool,
    provider: &ProviderConfig,
    username: &str,
    retries: u32,
) -> Result<Vec<StoryItem>, FetchError> {
    let mut last = FetchError::Network("未実行".into());
    for _ in 0..=retries {
        let proxy = pool.pick();
        let client = match pool.client(proxy.as_deref()) {
            Ok(c) => c,
            Err(e) => {
                if let Some(p) = &proxy {
                    pool.penalize(p);
                }
                last = e;
                continue;
            }
        };
        pool.throttle(proxy.as_deref(), provider.min_interval_ms).await;
        match try_fetch(&client, provider, username).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if !e.retryable() {
                    return Err(e);
                }
                match (&e, proxy.as_deref()) {
                    // 429: 相手の指定秒数だけ、その接続元を休ませる
                    (FetchError::RateLimited(s), k) => pool.penalize_for(
                        k.unwrap_or(""),
                        Duration::from_secs((*s).clamp(5, 600)),
                    ),
                    (_, Some(p)) => pool.penalize(p),
                    _ => {}
                }
                last = e;
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
        }
    }
    Err(last)
}

/// 画像・動画のバイナリ取得（プロキシ切替リトライ付き）
pub async fn download_bytes(pool: &Pool, url: &str, retries: u32) -> Result<Vec<u8>, FetchError> {
    if url.trim().is_empty() {
        return Err(FetchError::Parse("URLが空です".into()));
    }
    let mut last = FetchError::Network("未実行".into());
    for _ in 0..=retries {
        let proxy = pool.pick();
        let client = match pool.client(proxy.as_deref()) {
            Ok(c) => c,
            Err(e) => {
                if let Some(p) = &proxy {
                    pool.penalize(p);
                }
                last = e;
                continue;
            }
        };
        let err = match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                Ok(b) => return Ok(b.to_vec()),
                Err(e) => FetchError::Network(e.to_string()),
            },
            Ok(resp) => FetchError::Status(resp.status().as_u16()),
            Err(e) => FetchError::Network(e.to_string()),
        };
        if !err.retryable() {
            return Err(err);
        }
        if let Some(p) = &proxy {
            pool.penalize(p);
        }
        last = err;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg() -> ProviderConfig {
        ProviderConfig::default()
    }

    #[test]
    fn json_instagram_style_autodetect() {
        let root = json!({"data":[
            {"pk":"111","taken_at":1700000000,"media_type":2,
             "image_versions2":{"candidates":[
                {"url":"https://x/a_1080.jpg","width":1080,"height":1920},
                {"url":"https://x/a_320.jpg","width":320,"height":569}]},
             "video_versions":[
                {"url":"https://x/v720.mp4","width":720,"height":1280},
                {"url":"https://x/v1080.mp4","width":1080,"height":1920},
                {"url":"https://x/v480.mp4","width":480,"height":854}]},
            {"pk":222,"taken_at":1700000100,"media_type":1,
             "image_versions2":{"candidates":[{"url":"https://x/b.jpg","width":1080,"height":1920}]}}
        ]});
        let items = parse_json(&root, &cfg(), "u");
        assert_eq!(items.len(), 2);
        assert!(items[0].is_video && !items[1].is_video);
        assert_eq!(items[0].thumb_url, "https://x/a_320.jpg");
        assert_eq!(items[0].max_resolution(), (1080, 1920));
        let v = &items[0].videos;
        assert_eq!(pick_video(v, Pick::Quality(Quality::Best)).unwrap().width, 1080);
        assert_eq!(pick_video(v, Pick::Quality(Quality::Medium)).unwrap().width, 720);
        assert_eq!(pick_video(v, Pick::Quality(Quality::Low)).unwrap().width, 480);
        assert_eq!(pick_video(v, Pick::Exact(720, 1280)).unwrap().url, "https://x/v720.mp4");
    }

    #[test]
    fn json_custom_keys() {
        let root = json!({"data":[
            {"id":"a1","url":"https:\\/\\/c\\/x.mp4","thumbnail":"https://c/t.jpg","type":"video",
             "taken_at":"1700000000",
             "video_versions":[{"url":"https://c/hi.mp4","width":"1080","height":"1920"},
                               {"url":"https://c/lo.mp4","width":480,"height":854}]},
            {"id":"b2","url":"https://c/p.jpg","type":"image","taken_at":1700000050}
        ]});
        let items = parse_json(&root, &cfg(), "u");
        assert_eq!(items.len(), 2);
        assert!(items[0].is_video);
        assert_eq!(items[0].videos.len(), 2);
        assert_eq!(items[0].max_resolution(), (1080, 1920));
        assert!(!items[1].is_video);
        assert_eq!(items[1].image_url, "https://c/p.jpg");
        assert_eq!(items[1].thumb_url, "https://c/p.jpg");
    }

    #[test]
    fn html_regex_groups_variants_by_id() {
        let mut p = cfg();
        p.format = ResponseFormat::Html;
        p.html_regex = r#"data-id="(?P<id>\w+)" data-w="(?P<w>\d+)" data-h="(?P<h>\d+)" href="(?P<url>[^"]+)""#.into();
        let body = r#"
            <a data-id="s1" data-w="1080" data-h="1920" href="https://c/s1_hi.mp4?a=1&amp;b=2">
            <a data-id="s1" data-w="480" data-h="854" href="https://c/s1_lo.mp4">
            <a data-id="s2" data-w="1080" data-h="1920" href="https://c/s2.jpg">"#;
        let items = parse_html(body, &p, "u").unwrap();
        assert_eq!(items.len(), 2);
        assert!(items[0].is_video);
        assert_eq!(items[0].videos.len(), 2);
        assert_eq!(items[0].videos[0].url, "https://c/s1_hi.mp4?a=1&b=2");
        assert!(!items[1].is_video);
    }

    #[test]
    fn html_default_regex_and_bad_regex() {
        let mut p = cfg();
        p.format = ResponseFormat::Html;
        let items = parse_html(r#"<img src="https://c/a/b.jpg?x=1">"#, &p, "u").unwrap();
        assert_eq!(items.len(), 1);
        p.html_regex = "(".into();
        assert!(matches!(parse_html("", &p, "u"), Err(FetchError::Config(_))));
    }

    fn proxied(inner: &str, sig: &str) -> String {
        let d = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!(r#"{{"u":"{inner}","e":1795502999,"f":"x","i":1}}"#));
        format!("https://api.grabfy.com/api/proxy?d={d}&s={sig}")
    }

    #[test]
    fn grabfy_style_response() {
        let p = ProviderConfig::grabfy();
        let v1 = proxied("https://cdn.example/v/920_n.jpg?a=1", "sigA");
        let v2 = proxied("https://cdn.example/v/555_n.mp4?a=1", "sigB");
        let root = json!({"type":"profile","username":"u","isPrivate":false,"storiesPending":false,
            "stories":[
              {"type":"image","download":v1,"preview":v1},
              {"type":"video","download":v2,"play":v2,"preview":v1}]});
        let (items, pending) = parse_response(&root.to_string(), &p, "u").unwrap();
        assert!(!pending);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "920_n"); // 署名が変わってもIDは同じ
        assert!(!items[0].is_video);
        assert!(items[1].is_video);
        assert_eq!(items[1].id, "555_n");
        assert_eq!(items[1].videos.len(), 1);
        assert_eq!(items[1].videos[0].url, v2);
        // 署名違いでも同じID
        let v1b = proxied("https://cdn.example/v/920_n.jpg?a=1", "otherSig");
        assert_eq!(url_id(&v1), url_id(&v1b));
    }

    #[test]
    fn grabfy_pending_and_private() {
        let p = ProviderConfig::grabfy();
        let pend = json!({"stories":[],"storiesPending":true}).to_string();
        let (items, pending) = parse_response(&pend, &p, "u").unwrap();
        assert!(items.is_empty() && pending);
        let private = json!({"profile":{"isPrivate":true},"stories":[]}).to_string();
        assert!(matches!(parse_response(&private, &p, "u"), Err(FetchError::Private)));
    }

    #[test]
    fn proxy_normalize() {
        assert_eq!(normalize_proxy("1.2.3.4:8080").unwrap(), "http://1.2.3.4:8080");
        assert_eq!(
            normalize_proxy("socks5://u:p@1.2.3.4:1080").unwrap(),
            "socks5://u:p@1.2.3.4:1080"
        );
        assert!(normalize_proxy("  # comment").is_none());
    }
}
