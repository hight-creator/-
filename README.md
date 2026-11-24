# Instagram 公開ストーリー GUI ダウンローダー（Rust + egui）

WebView2・Node.js・Python 不要。`cargo build --release` で完全単体の `.exe` が1つできます。
設定・アカウント・プロキシは **exeと同じフォルダの `config.json`** に自動保存されます。

## 1. Windows 10 での環境構築（最短）

PowerShell（管理者でなくてOK、winget が使える Windows 10 21H2 以降）で:

```powershell
# ① C++ ビルドツール（MSVCリンカ）
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"

# ② Rust
winget install Rustlang.Rustup
rustup default stable-x86_64-pc-windows-msvc
```

PowerShell を開き直してから、このフォルダで:

```powershell
cargo build --release
```

出力: `target\release\ig-story-dl.exe`（これ1ファイルだけ配布・移動すればOK）

> `.cargo\config.toml` で `+crt-static` を指定しているため、VC++再頒布パッケージも不要です。
> HTTPS は Windows 標準の Schannel を使うので OpenSSL も不要です。

## 2. プロジェクト構成

```
ig-story-dl/
├─ Cargo.toml            依存クレート + サイズ最適化 (lto / strip / opt-level=z / panic=abort)
├─ .cargo/config.toml    CRT静的リンク
└─ src/
   ├─ main.rs            起動・ウィンドウ設定
   ├─ app.rs             GUI（サイドバー / サムネ一覧 / プレビュー / 設定 / ログ）
   ├─ config.rs          exe隣の config.json 読み書き
   ├─ scraper.rs         ★取得・解析ロジック（Instagram仕様変更時はここだけ直す）
   └─ downloader.rs      保存処理（ファイル名・フォルダ振り分け）
```

## 3. 使い方

1. 左サイドバーにユーザー名（`@`付き・URL・複数可）を入れて「追加」。タグも付けられます。
2. 上部の「登録アカウントをすべて一括チェック」で全アカウントを並列取得。
3. サムネをクリックで拡大プレビュー。動画は解像度（例 1080x1920 / 720x1280 / 480x854）を選んで保存できます。
4. チェックを入れて「選択をダウンロード」、または「全件ダウンロード」（既定画質は上部のコンボボックス）。
5. 「設定・プロキシ」タブで保存先・同時接続数・リトライ・プロキシリストを設定。

保存先: `保存先/{username}/{username}_{YYYYMMDD_HHMMSS}_{id}.mp4|jpg`

## 4. 画質の選び方（`scraper::pick_video`）

| 指定 | 動作 |
|---|---|
| Best | `video_versions` のうち最大解像度 |
| Medium | 短辺が 720px に最も近いもの |
| Low | 最小解像度 |
| 個別指定 | 一致する解像度（なければ最大） |

## 5. 取得方式と「取得先」の設定（最重要）

Instagram本体はストーリー取得にログインを要求します。そのため本ソフトは、
**ログイン不要で公開ストーリーのCDNリンクを返してくれる無料Webビューア型サービスの裏側API** に、
`reqwest` で（プロキシローテーション付きで）直接リクエストして、メディアURL・解像度・投稿時刻を取り出します。

- 取得先URL・送信内容・応答の抽出ルールは **コードではなく `config.json` / 設定タブの「取得先」** にあります。
  相手の仕様が変わったら、そこを書き換えるだけで追従できます。
- **初期設定は grabfy.com 用です**（同サイトの `/js/grabfy.js` を読んで設定。`POST https://api.grabfy.com/api/resolve`、本文 `{"url":"ユーザー名"}`、
  署名・トークンなし、`storiesPending` が true の間は `{"url":"ユーザー名","storiesOnly":true}` でポーリング）。
  実機での動作は未確認です。設定タブの「Grabfy の設定に戻す」でいつでも初期値に戻せます。
  このサイトは1項目につき1つのURLだけを返すため、**画質選択（video_versions）は効きません**（常に元画質）。
- 別のサービスを使う場合は、そのサービスのブラウザ開発者ツール（Network タブ）で、ユーザー名を入力した時の
  通信（URL・メソッド・フォーム内容・ヘッダ・応答）を確認し、設定タブへ転記してください。
  - 応答がJSON → 「配列のパス」「各キー名」を合わせる（`image_versions2`/`video_versions` 形式なら自動認識）
  - 応答がHTML → 「正規表現」を合わせる（名前付きグループ `url`必須 / `thumb` `type` `w` `h` `time` `id`。
    同じ `id` のマッチは動画の画質違いとしてまとめます）
- 修正が必要になりうるコードは `src/scraper.rs` の `try_fetch`（リクエスト組み立て）と `parse_json` / `parse_html`（抽出）だけです。

### できないこと・注意
- 相手サービスが Cloudflare/Turnstile・CAPTCHA・事前のトークン取得（CSRF）を要求する場合、`reqwest` 単体では通りません。
  その場合は「追加ヘッダ」に値を入れても突破できないことがあります。
- 無料ビューア型サービスは、多くがサービス側のログイン済みアカウントやプロキシで動いており、突然仕様変更・停止・規約変更が起こります。
- 取得したコンテンツの権利は投稿者にあります。私的利用の範囲で、各サービス・Instagramの利用規約に留意してください。
