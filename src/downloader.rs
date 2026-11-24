//! 保存処理（ファイル名・フォルダ振り分け）

use crate::scraper::{self, Pick, Pool, StoryItem};
use chrono::{Local, TimeZone};
use std::path::{Path, PathBuf};

pub fn format_time(ts: i64, fmt: &str) -> String {
    if ts <= 0 {
        return "notime".to_string(); // 投稿時刻が取れないサイト用
    }
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|d| d.format(fmt).to_string())
        .unwrap_or_else(|| ts.to_string())
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn image_ext(url: &str) -> &'static str {
    let path = url.split('?').next().unwrap_or(url).to_lowercase();
    if path.ends_with(".png") {
        "png"
    } else if path.ends_with(".webp") {
        "webp"
    } else {
        "jpg"
    }
}

/// 1件保存する。保存先: `dir/{username}/{username}_{timestamp}_{id}.{ext}`（振り分けOFFなら dir 直下）
pub async fn download_item(
    pool: &Pool,
    item: &StoryItem,
    pick: Pick,
    dir: &Path,
    organize: bool,
    retries: u32,
) -> Result<PathBuf, String> {
    let (url, ext) = if item.is_video {
        let v = scraper::pick_video(&item.videos, pick).ok_or("動画URLが見つかりません")?;
        (v.url, "mp4")
    } else {
        (item.image_url.clone(), image_ext(&item.image_url))
    };

    let mut folder = dir.to_path_buf();
    if organize {
        folder.push(sanitize(&item.username));
    }
    tokio::fs::create_dir_all(&folder)
        .await
        .map_err(|e| format!("フォルダ作成失敗: {e}"))?;

    let name = format!(
        "{}_{}_{}.{}",
        sanitize(&item.username),
        format_time(item.taken_at, "%Y%m%d_%H%M%S"),
        sanitize(&item.id),
        ext
    );
    let path = folder.join(name);
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Ok(path); // 保存済みはスキップ
    }

    let bytes = scraper::download_bytes(pool, &url, retries)
        .await
        .map_err(|e| e.to_string())?;
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|e| format!("保存失敗: {e}"))?;
    Ok(path)
}
