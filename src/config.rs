//! ポータブル設定管理：EXEと同じフォルダの config.json に読み書きする。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Best,
    Medium,
    Low,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Best, Quality::Medium, Quality::Low];

    pub fn label(&self) -> &'static str {
        match self {
            Quality::Best => "最高画質（Best）",
            Quality::Medium => "標準（Medium）",
            Quality::Low => "低画質・軽量（Low）",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Post,
    Get,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseFormat {
    Json,
    Html,
}

/// 取得先（Webビューア型サービスの裏側API）の仕様。
/// 相手側の仕様変更時は、コードではなくここ（config.json / 設定タブ）を書き換えるだけで追従できる。
/// `{username}` は URL・本文・ヘッダ値の中で登録ユーザー名に置換される。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct ProviderConfig {
    pub url: String,
    pub method: HttpMethod,
    pub content_type: String,
    /// POST本文。例: `username={username}` / `{"username":"{username}"}`
    pub body_template: String,
    /// 追加ヘッダ（`Name: value` を1件ずつ）。Origin / Referer / x-requested-with など
    pub headers: Vec<String>,
    pub format: ResponseFormat,

    // ── JSON形式のとき ──
    /// ストーリー配列へのJSONポインタ。例: `/data` `/result/items`。空ならルートが配列
    pub items_path: String,
    pub key_id: String,
    pub key_thumb: String,
    /// メディア本体URLのキー
    pub key_media: String,
    pub key_time: String,
    /// "video" を含めば動画扱い
    pub key_type: String,
    /// 解像度別動画の配列キー（要素は url / width / height）
    pub key_versions: String,

    // ── HTML形式のとき ──
    /// 1マッチ=1メディア。名前付きグループ: url(必須) thumb type w h time id
    /// 同じ id のマッチは動画の画質違いとしてまとめる
    pub html_regex: String,

    /// 応答にこの文字列が含まれ、かつ0件なら「非公開アカウント」と表示
    pub private_marker: String,
    /// 非公開を示す真偽値へのJSONポインタ（カンマ区切りで複数可）。例: `/isPrivate,/profile/isPrivate`
    pub private_path: String,

    /// 「準備中」を示す真偽値へのJSONポインタ。trueかつ0件なら、下の本文でポーリングする。空なら無効
    pub pending_path: String,
    /// ポーリング用のPOST本文（`{username}` 置換可）。空ならポーリングしない
    pub poll_body_template: String,
    /// ポーリングの最大回数（間隔は1秒から8秒まで段階的に延びる）
    pub poll_max: u32,

    /// 同じ接続元（各プロキシ／直接接続ごと）への最小リクエスト間隔(ミリ秒)。
    /// 閾値超過で使えなくなるのを避けるため、制限に合わせて大きめにする。
    pub min_interval_ms: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            method: HttpMethod::Post,
            content_type: "application/x-www-form-urlencoded; charset=UTF-8".into(),
            body_template: "username={username}".into(),
            headers: Vec::new(),
            format: ResponseFormat::Json,
            items_path: "/data".into(),
            key_id: "id".into(),
            key_thumb: "thumbnail".into(),
            key_media: "url".into(),
            key_time: "taken_at".into(),
            key_type: "type".into(),
            key_versions: "video_versions".into(),
            html_regex: r#"(?P<url>https?://[^"'\s<>\\]+?\.(?:mp4|jpg|jpeg|webp)[^"'\s<>\\]*)"#
                .into(),
            private_marker: "private account".into(),
            private_path: String::new(),
            pending_path: String::new(),
            poll_body_template: String::new(),
            poll_max: 12,
            min_interval_ms: 2500,
        }
    }
}

impl ProviderConfig {
    /// grabfy.com の /js/grabfy.js から読み取った仕様（署名・トークンなし、storiesPending のポーリングあり）。
    /// 実機での動作は未確認。相手の仕様が変わったら設定タブで直す。
    pub fn grabfy() -> Self {
        Self {
            url: "https://api.grabfy.com/api/resolve".into(),
            method: HttpMethod::Post,
            content_type: "application/json".into(),
            body_template: r#"{"url":"{username}"}"#.into(),
            headers: vec![
                "Origin: https://grabfy.com".into(),
                "Referer: https://grabfy.com/".into(),
            ],
            format: ResponseFormat::Json,
            items_path: "/stories".into(),
            key_thumb: "preview".into(),
            key_media: "download".into(),
            key_type: "type".into(),
            private_marker: String::new(),
            private_path: "/isPrivate,/profile/isPrivate".into(),
            pending_path: "/storiesPending".into(),
            poll_body_template: r#"{"url":"{username}","storiesOnly":true}"#.into(),
            min_interval_ms: 3000,
            ..Self::default()
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Account {
    pub username: String,
    #[serde(default)]
    pub tag: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub accounts: Vec<Account>,
    /// 1行1件。例: ip:port / user:pass@ip:port / socks5://user:pass@ip:port
    pub proxies: Vec<String>,
    pub use_proxy: bool,
    pub quality: Quality,
    pub save_dir: String,
    pub organize_by_user: bool,
    pub max_concurrency: usize,
    pub max_retries: u32,
    pub hide_empty: bool,
    pub provider: ProviderConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            proxies: Vec::new(),
            use_proxy: false,
            quality: Quality::Best,
            save_dir: exe_dir().join("downloads").to_string_lossy().to_string(),
            organize_by_user: true,
            max_concurrency: 4,
            max_retries: 3,
            hide_empty: true,
            provider: ProviderConfig::grabfy(),
        }
    }
}

/// 実行ファイルのあるフォルダ
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_path() -> PathBuf {
    exe_dir().join("config.json")
}

impl Config {
    pub fn load() -> Self {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let s = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(config_path(), s)
    }

    /// 空白・カンマ・改行区切りで複数登録できる。@付き・プロフィールURLも可。追加できた件数を返す。
    pub fn add_accounts(&mut self, raw: &str, tag: &str) -> usize {
        let mut added = 0;
        for part in raw.split(|c: char| c.is_whitespace() || c == ',') {
            if let Some(name) = parse_username(part) {
                if !self.accounts.iter().any(|a| a.username == name) {
                    self.accounts.push(Account {
                        username: name,
                        tag: tag.trim().to_string(),
                    });
                    added += 1;
                }
            }
        }
        added
    }
}

pub fn parse_username(s: &str) -> Option<String> {
    let mut t = s.trim().trim_start_matches('@');
    for p in ["https://", "http://"] {
        if let Some(r) = t.strip_prefix(p) {
            t = r;
        }
    }
    if let Some(r) = t.strip_prefix("www.") {
        t = r;
    }
    if let Some(r) = t.strip_prefix("instagram.com/") {
        t = r;
    }
    let t = t.trim_start_matches('@');
    let t = t
        .split(|c: char| c == '/' || c == '?' || c == '#')
        .next()
        .unwrap_or("");
    if t.is_empty()
        || t.len() > 30
        || !t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
    {
        return None;
    }
    Some(t.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_parsing() {
        assert_eq!(parse_username("@Foo_Bar"), Some("foo_bar".into()));
        assert_eq!(
            parse_username("https://www.instagram.com/foo.bar/?hl=ja"),
            Some("foo.bar".into())
        );
        assert_eq!(parse_username("bad name!"), None);
        assert_eq!(parse_username(""), None);
    }

    #[test]
    fn add_dedup() {
        let mut c = Config::default();
        assert_eq!(c.add_accounts("a, @b  a\nc", "t"), 3);
        assert_eq!(c.accounts.len(), 3);
    }
}
