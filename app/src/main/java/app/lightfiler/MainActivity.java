package app.lightfiler;

import android.Manifest;
import android.app.Activity;
import android.app.AlertDialog;
import android.app.ProgressDialog;
import android.content.ActivityNotFoundException;
import android.content.DialogInterface;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.content.res.Configuration;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Rect;
import android.graphics.Typeface;
import android.media.MediaMetadataRetriever;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.StatFs;
import android.provider.DocumentsContract;
import android.text.InputType;
import android.text.TextUtils;
import android.util.DisplayMetrics;
import android.util.LruCache;
import android.view.Gravity;
import android.view.Menu;
import android.view.MenuItem;
import android.view.View;
import android.view.ViewGroup;
import android.widget.AbsListView;
import android.widget.AdapterView;
import android.widget.BaseAdapter;
import android.widget.Button;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.GridView;
import android.widget.HorizontalScrollView;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.RadioButton;
import android.widget.RadioGroup;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import java.io.File;
import java.io.IOException;
import java.text.SimpleDateFormat;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.Date;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Locale;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;

public class MainActivity extends Activity {

    // ================= データ型 =================
    static final class Item {
        File f;
        String name, key;
        boolean dir, video;
        long size, mod;
    }

    static final class Meta {
        int w, h;
        long dur;
        boolean ok;
        Bitmap thumb;
    }

    interface Ok {
        void accept(String s);
    }

    interface Job {
        void run(Fs.Prog p) throws Exception;
    }

    private static final String[] VIDEO_EXT = {"mp4", "mkv", "avi", "mov", "3gp", "3g2", "webm",
            "flv", "wmv", "m4v", "ts", "mts", "m2ts", "mpg", "mpeg", "vob", "ogv"};
    private static final int REQ_PERM = 1, REQ_TREE = 2;
    private static final int THUMB_W = 320, THUMB_H = 240;

    // ================= 状態 =================
    private Fs fs;
    private SharedPreferences sp;
    private File cwd;
    private ArrayList<Item> items = new ArrayList<>();
    private final HashSet<String> sel = new HashSet<>();
    private final ArrayList<File> clip = new ArrayList<>();
    private boolean clipCut, showHidden, searching, grid, sortAsc;
    private int sortBy;
    private String searchQuery = "";
    private int cellH;

    // UI
    private GridView lv;
    private LinearLayout pathRow, actionBar, pasteBar;
    private HorizontalScrollView pathScroll;
    private TextView storTv, selTv;
    private ProgressBar storBar;
    private Button btnView, btnInt, btnSd, btnRename, btnDetail, btnPaste;
    private Adapter adapter;

    // 非同期・キャッシュ
    private final Handler ui = new Handler(Looper.getMainLooper());
    private final ExecutorService metaExec = Executors.newFixedThreadPool(2);
    private final ExecutorService dirExec = Executors.newSingleThreadExecutor();
    private final HashMap<String, Meta> vcache = new HashMap<>();
    private final HashSet<String> noThumb = new HashSet<>();
    private final HashSet<String> pending = new HashSet<>();
    private final HashMap<String, long[]> dstats = new HashMap<>();
    private final HashSet<String> dirPending = new HashSet<>();
    private final HashMap<String, Integer> positions = new HashMap<>();
    private final LruCache<String, Bitmap> thumbs = new LruCache<String, Bitmap>(
            (int) Math.max(2 * 1024 * 1024, Runtime.getRuntime().maxMemory() / 10)) {
        @Override
        protected int sizeOf(String k, Bitmap b) {
            return b.getByteCount();
        }
    };
    private final SimpleDateFormat df = new SimpleDateFormat("yyyy/MM/dd HH:mm", Locale.getDefault());
    private volatile int gen, firstVis, lastVis = 40;
    private boolean refreshPending;

    // ================= ライフサイクル =================
    @Override
    protected void onCreate(Bundle b) {
        super.onCreate(b);
        fs = new Fs(this);
        sp = getSharedPreferences("p", 0);
        sortBy = sp.getInt("sort", 0);
        sortAsc = sp.getBoolean("asc", true);
        showHidden = sp.getBoolean("hidden", false);
        grid = sp.getBoolean("grid", false);

        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);

        // --- 上部ボタン ---
        LinearLayout tb = new LinearLayout(this);
        tb.setOrientation(LinearLayout.HORIZONTAL);
        btnView = tbBtn("表示切替");
        btnInt = tbBtn("内部ストレージ");
        btnSd = tbBtn("SDカード");
        tb.addView(btnView, new LinearLayout.LayoutParams(0, -2, 1f));
        tb.addView(btnInt, new LinearLayout.LayoutParams(0, -2, 1f));
        tb.addView(btnSd, new LinearLayout.LayoutParams(0, -2, 1f));
        root.addView(tb, new LinearLayout.LayoutParams(-1, -2));
        btnView.setOnClickListener(v -> {
            grid = !grid;
            sp.edit().putBoolean("grid", grid).apply();
            applyMode();
        });
        btnInt.setOnClickListener(v -> open(fs.primary, false));
        btnSd.setOnClickListener(v -> openSd());

        // --- 容量表示 ---
        storTv = new TextView(this);
        storTv.setTextSize(12);
        storTv.setSingleLine(true);
        storTv.setEllipsize(TextUtils.TruncateAt.END);
        storTv.setPadding(dp(10), dp(4), dp(10), 0);
        root.addView(storTv, new LinearLayout.LayoutParams(-1, -2));
        storBar = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
        storBar.setMax(100);
        LinearLayout.LayoutParams bp = new LinearLayout.LayoutParams(-1, dp(4));
        bp.setMargins(dp(10), dp(2), dp(10), dp(4));
        root.addView(storBar, bp);
        selTv = new TextView(this);
        selTv.setTextSize(13);
        selTv.setTypeface(null, Typeface.BOLD);
        selTv.setBackgroundColor(0xFFCCE4FF);
        selTv.setPadding(dp(10), dp(4), dp(10), dp(4));
        selTv.setVisibility(View.GONE);
        root.addView(selTv, new LinearLayout.LayoutParams(-1, -2));

        // --- パス(パンくず) ---
        pathScroll = new HorizontalScrollView(this);
        pathScroll.setHorizontalScrollBarEnabled(false);
        pathScroll.setBackgroundColor(0xFFE8E8E8);
        pathRow = new LinearLayout(this);
        pathRow.setOrientation(LinearLayout.HORIZONTAL);
        pathRow.setGravity(Gravity.CENTER_VERTICAL);
        pathScroll.addView(pathRow, new FrameLayout.LayoutParams(-2, -2));
        root.addView(pathScroll, new LinearLayout.LayoutParams(-1, -2));

        // --- 一覧/サムネ ---
        lv = new GridView(this);
        adapter = new Adapter();
        lv.setFastScrollEnabled(true);
        lv.setStretchMode(GridView.STRETCH_COLUMN_WIDTH);
        root.addView(lv, new LinearLayout.LayoutParams(-1, 0, 1f));

        // --- 下部: 選択時の操作バー ---
        actionBar = new LinearLayout(this);
        actionBar.setOrientation(LinearLayout.VERTICAL);
        actionBar.setBackgroundColor(0xFFF0F0F0);
        LinearLayout r1 = new LinearLayout(this);
        LinearLayout r2 = new LinearLayout(this);
        addBar(r1, "コピー", v -> {
            clip.clear();
            clip.addAll(selected());
            clipCut = false;
            toast("コピー: " + clip.size() + " 件 → 貼り付け先を開いて「ここに貼り付け」");
            clearSel();
        });
        addBar(r1, "切り取り", v -> {
            clip.clear();
            clip.addAll(selected());
            clipCut = true;
            toast("切り取り: " + clip.size() + " 件 → 貼り付け先を開いて「ここに貼り付け」");
            clearSel();
        });
        addBar(r1, "削除", v -> confirmDelete(selected()));
        btnRename = addBar(r1, "名前変更", v -> {
            ArrayList<File> s = selected();
            if (s.size() == 1) rename(s.get(0));
        });
        addBar(r2, "共有", v -> share(selected()));
        addBar(r2, "全選択", v -> {
            for (Item it : items) sel.add(it.f.getPath());
            updateSelUi();
        });
        btnDetail = addBar(r2, "詳細", v -> {
            ArrayList<File> s = selected();
            if (s.size() == 1) detail(s.get(0));
        });
        addBar(r2, "選択解除", v -> clearSel());
        actionBar.addView(r1, new LinearLayout.LayoutParams(-1, -2));
        actionBar.addView(r2, new LinearLayout.LayoutParams(-1, -2));
        actionBar.setVisibility(View.GONE);
        root.addView(actionBar, new LinearLayout.LayoutParams(-1, -2));

        // --- 下部: 貼り付けバー ---
        pasteBar = new LinearLayout(this);
        pasteBar.setBackgroundColor(0xFFF0F0F0);
        btnPaste = addBar(pasteBar, "ここに貼り付け", v -> paste());
        ((LinearLayout.LayoutParams) btnPaste.getLayoutParams()).weight = 2f;
        addBar(pasteBar, "取消", v -> {
            clip.clear();
            updateSelUi();
        });
        pasteBar.setVisibility(View.GONE);
        root.addView(pasteBar, new LinearLayout.LayoutParams(-1, -2));

        setContentView(root);

        lv.setOnScrollListener(new AbsListView.OnScrollListener() {
            @Override
            public void onScrollStateChanged(AbsListView v, int s) {
                if (s == SCROLL_STATE_IDLE) adapter.notifyDataSetChanged();
            }

            @Override
            public void onScroll(AbsListView v, int first, int visible, int total) {
                firstVis = first;
                lastVis = first + visible;
            }
        });
        lv.setOnItemClickListener(new AdapterView.OnItemClickListener() {
            @Override
            public void onItemClick(AdapterView<?> p, View v, int i, long id) {
                Item it = items.get(i);
                if (!sel.isEmpty()) toggle(it);
                else if (it.dir) open(it.f, false);
                else openFile(it.f);
            }
        });
        lv.setOnItemLongClickListener(new AdapterView.OnItemLongClickListener() {
            @Override
            public boolean onItemLongClick(AdapterView<?> p, View v, int i, long id) {
                toggle(items.get(i));
                return true;
            }
        });
        applyMode();

        if (Build.VERSION.SDK_INT >= 23
                && checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[]{Manifest.permission.READ_EXTERNAL_STORAGE,
                    Manifest.permission.WRITE_EXTERNAL_STORAGE}, REQ_PERM);
        } else {
            start();
        }
    }

    private Button tbBtn(String t) {
        Button b = new Button(this);
        b.setText(t);
        b.setTextSize(12);
        b.setSingleLine(true);
        b.setPadding(dp(2), 0, dp(2), 0);
        b.setMinHeight(dp(40));
        b.setMinWidth(0);
        return b;
    }

    private Button addBar(LinearLayout row, String t, View.OnClickListener l) {
        Button b = tbBtn(t);
        b.setOnClickListener(l);
        row.addView(b, new LinearLayout.LayoutParams(0, -2, 1f));
        return b;
    }

    @Override
    public void onRequestPermissionsResult(int rq, String[] perms, int[] res) {
        start();
    }

    @Override
    public void onConfigurationChanged(Configuration c) {
        super.onConfigurationChanged(c);
        applyMode();
    }

    private void start() {
        String last = sp.getString("last", null);
        File d = last != null ? new File(last) : fs.primary;
        if (!d.isDirectory() || !d.canRead()) d = fs.primary;
        open(d, false);
    }

    @Override
    protected void onStop() {
        super.onStop();
        if (cwd != null) sp.edit().putString("last", cwd.getPath()).apply();
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        metaExec.shutdownNow();
        dirExec.shutdownNow();
    }

    @Override
    public void onBackPressed() {
        if (!sel.isEmpty()) {
            clearSel();
        } else if (searching) {
            open(cwd, true);
        } else if (!goUp()) {
            super.onBackPressed();
        }
    }

    private boolean goUp() {
        if (cwd == null) return false;
        File par = cwd.getParentFile();
        if (par == null || cwd.equals(fs.primary) || fs.sdRoots.contains(cwd) || !par.canRead()) return false;
        open(par, true);
        return true;
    }

    private void openSd() {
        if (fs.sdRoots.isEmpty()) {
            toast("SDカードが見つかりません");
        } else if (fs.sdRoots.size() == 1) {
            open(fs.sdRoots.get(0), false);
        } else {
            final ArrayList<File> r = fs.sdRoots;
            String[] n = new String[r.size()];
            for (int i = 0; i < n.length; i++) n[i] = r.get(i).getPath();
            new AlertDialog.Builder(this).setTitle("SDカード").setItems(n, (d, w) -> open(r.get(w), false)).show();
        }
    }

    // ================= 表示モード =================
    private void applyMode() {
        DisplayMetrics dm = getResources().getDisplayMetrics();
        int pos = lv.getFirstVisiblePosition();
        if (grid) {
            int cols = dm.widthPixels > dm.heightPixels ? 5 : 3;
            int gap = dp(2);
            int colW = (dm.widthPixels - gap * (cols - 1)) / cols;
            cellH = colW * 3 / 4;
            lv.setNumColumns(cols);
            lv.setHorizontalSpacing(gap);
            lv.setVerticalSpacing(gap);
        } else {
            lv.setNumColumns(1);
            lv.setHorizontalSpacing(0);
            lv.setVerticalSpacing(0);
        }
        lv.setAdapter(adapter);
        lv.setSelection(pos);
        btnView.setText(grid ? "一覧表示へ" : "サムネ表示へ");
    }

    // ================= 一覧の構築 =================
    private Item mk(File f) {
        Item it = new Item();
        it.f = f;
        it.name = f.getName();
        it.dir = f.isDirectory();
        it.size = it.dir ? 0 : f.length();
        it.mod = f.lastModified();
        it.video = !it.dir && isVideo(it.name);
        it.key = f.getPath() + "|" + it.mod + "|" + it.size;
        return it;
    }

    private static String ext(String n) {
        int d = n.lastIndexOf('.');
        return d >= 0 ? n.substring(d + 1) : "";
    }

    private static boolean isVideo(String n) {
        String e = ext(n).toLowerCase(Locale.ROOT);
        for (String v : VIDEO_EXT) if (v.equals(e)) return true;
        return false;
    }

    private long pix(Item it) {
        Meta m = vcache.get(it.key);
        return m == null ? 0 : (long) m.w * m.h;
    }

    private long durOf(Item it) {
        Meta m = vcache.get(it.key);
        return m == null ? 0 : m.dur;
    }

    private final Comparator<Item> cmp = new Comparator<Item>() {
        @Override
        public int compare(Item a, Item b) {
            if (a.dir != b.dir) return a.dir ? -1 : 1;
            int r;
            switch (sortBy) {
                case 1:
                    r = Long.compare(a.mod, b.mod);
                    break;
                case 2:
                    r = Long.compare(a.size, b.size);
                    break;
                case 3:
                    r = ext(a.name).compareToIgnoreCase(ext(b.name));
                    break;
                case 4:
                    r = Long.compare(pix(a), pix(b));
                    break;
                case 5:
                    r = Long.compare(durOf(a), durOf(b));
                    break;
                default:
                    r = 0;
            }
            if (r != 0) return sortAsc ? r : -r;
            int n = a.name.compareToIgnoreCase(b.name);
            return (sortBy == 0 && !sortAsc) ? -n : n;
        }
    };

    private void sortItems() {
        Collections.sort(items, cmp);
        adapter.notifyDataSetChanged();
    }

    /** 解像度/長さでの並べ替え時、未取得の動画情報をまとめて取得してから並べ直す */
    private void ensureSortMeta() {
        if (sortBy < 4) return;
        final ArrayList<Item> need = new ArrayList<>();
        for (Item it : items) if (it.video && !vcache.containsKey(it.key)) need.add(it);
        if (need.isEmpty()) return;
        final HashMap<String, Meta> got = new HashMap<>();
        runJob("動画情報を取得中(並べ替え用)", p -> {
            for (Item it : need) {
                if (p.cancelled()) break;
                p.file(it.name);
                Meta m = probe(it.f, false);
                synchronized (got) {
                    got.put(it.key, m);
                }
            }
        }, () -> {
            synchronized (got) {
                vcache.putAll(got);
            }
            sortItems();
        });
    }

    private void open(File dir, boolean restore) {
        File[] fl = dir.listFiles();
        if (fl == null) {
            toast("開けません: " + dir.getPath());
            return;
        }
        if (cwd != null && !searching) positions.put(cwd.getPath(), lv.getFirstVisiblePosition());
        searching = false;
        gen++;
        pending.clear();
        dirPending.clear();
        sel.clear();
        cwd = dir;
        ArrayList<Item> n = new ArrayList<>(fl.length);
        for (File f : fl) {
            if (!showHidden && f.getName().startsWith(".")) continue;
            n.add(mk(f));
        }
        items = n;
        Collections.sort(items, cmp);
        firstVis = 0;
        lastVis = 40;
        buildPath();
        updateStorage();
        updateSelUi();
        Integer p = restore ? positions.get(dir.getPath()) : null;
        lv.setSelection(p != null ? p : 0);
        ensureSortMeta();
    }

    private void reload() {
        dstats.clear();
        open(cwd, true);
    }

    // ================= パンくず・容量表示 =================
    private static boolean under(String p, File r) {
        String rp = r.getPath();
        return p.equals(rp) || p.startsWith(rp + "/");
    }

    private void buildPath() {
        pathRow.removeAllViews();
        if (searching) {
            pathRow.addView(seg("検索: " + searchQuery + "  (戻るキーで元のフォルダへ)", null, true));
            return;
        }
        String p = cwd.getPath();
        File base = null;
        String baseLabel = null;
        if (under(p, fs.primary)) {
            base = fs.primary;
            baseLabel = "内部ストレージ";
        } else {
            File r = fs.rootOf(cwd);
            if (r != null) {
                base = r;
                baseLabel = "SDカード";
            }
        }
        ArrayList<String> labels = new ArrayList<>();
        ArrayList<String> paths = new ArrayList<>();
        String rest;
        String cur;
        if (base != null) {
            labels.add(baseLabel);
            paths.add(base.getPath());
            rest = p.substring(base.getPath().length());
            cur = base.getPath();
        } else {
            labels.add("/");
            paths.add("/");
            rest = p;
            cur = "";
        }
        for (String s : rest.split("/")) {
            if (s.isEmpty()) continue;
            cur = cur + "/" + s;
            labels.add(s);
            paths.add(cur);
        }
        for (int i = 0; i < labels.size(); i++) {
            if (i > 0) {
                TextView sep = new TextView(this);
                sep.setText("›");
                sep.setTextColor(0xFF888888);
                pathRow.addView(sep);
            }
            pathRow.addView(seg(labels.get(i), paths.get(i), i == labels.size() - 1));
        }
        pathScroll.post(() -> pathScroll.fullScroll(View.FOCUS_RIGHT));
    }

    private TextView seg(String text, final String path, boolean last) {
        TextView t = new TextView(this);
        t.setText(text);
        t.setTextSize(14);
        t.setPadding(dp(8), dp(9), dp(8), dp(9));
        t.setSingleLine(true);
        if (last) {
            t.setTypeface(null, Typeface.BOLD);
            t.setTextColor(0xFF202020);
        } else {
            t.setTextColor(0xFF1A5FB4);
        }
        if (path != null) {
            t.setOnClickListener(v -> {
                if (!path.equals(cwd.getPath()) || searching) open(new File(path), false);
            });
        }
        return t;
    }

    private void updateStorage() {
        File root;
        String label;
        String p = cwd.getPath();
        if (under(p, fs.primary)) {
            root = fs.primary;
            label = "内部ストレージ";
        } else {
            File r = fs.rootOf(cwd);
            if (r != null) {
                root = r;
                label = "SDカード";
            } else {
                root = cwd;
                label = "ストレージ";
            }
        }
        btnInt.setTypeface(null, label.startsWith("内部") ? Typeface.BOLD : Typeface.NORMAL);
        btnSd.setTypeface(null, label.equals("SDカード") ? Typeface.BOLD : Typeface.NORMAL);
        try {
            StatFs s = new StatFs(root.getPath());
            long tot = s.getTotalBytes(), av = s.getAvailableBytes(), used = tot - av;
            storTv.setText(label + "  使用 " + fmtSize(used) + " / 合計 " + fmtSize(tot)
                    + "  (空き " + fmtSize(av) + ")");
            storBar.setProgress(tot > 0 ? (int) (used * 100 / tot) : 0);
        } catch (Exception e) {
            storTv.setText(label + "  容量を取得できません");
            storBar.setProgress(0);
        }
    }

    /** 選択中の合計容量表示を更新 */
    private void updateBar() {
        if (sel.isEmpty()) {
            selTv.setVisibility(View.GONE);
            return;
        }
        long total = 0;
        boolean calc = false;
        int n = 0;
        for (Item it : items) {
            String path = it.f.getPath();
            if (!sel.contains(path)) continue;
            n++;
            if (it.dir) {
                long[] st = dstats.get(path);
                if (st == null) {
                    calc = true;
                    queueDir(path, -1);
                } else {
                    total += st[0];
                }
            } else {
                total += it.size;
            }
        }
        selTv.setText("選択 " + n + " 件   合計 " + fmtSize(total) + (calc ? " (計算中…)" : ""));
        selTv.setVisibility(View.VISIBLE);
    }

    // ================= 選択 =================
    private void toggle(Item it) {
        String p = it.f.getPath();
        if (!sel.remove(p)) sel.add(p);
        updateSelUi();
    }

    private void clearSel() {
        sel.clear();
        updateSelUi();
    }

    private void updateSelUi() {
        boolean s = !sel.isEmpty();
        actionBar.setVisibility(s ? View.VISIBLE : View.GONE);
        pasteBar.setVisibility(!s && !clip.isEmpty() ? View.VISIBLE : View.GONE);
        btnRename.setEnabled(sel.size() == 1);
        btnDetail.setEnabled(sel.size() == 1);
        if (!clip.isEmpty()) btnPaste.setText("ここに" + (clipCut ? "移動" : "コピー") + " (" + clip.size() + "件)");
        updateBar();
        adapter.notifyDataSetChanged();
    }

    private ArrayList<File> selected() {
        ArrayList<File> r = new ArrayList<>();
        for (Item it : items) if (sel.contains(it.f.getPath())) r.add(it.f);
        return r;
    }

    // ================= 外部アプリ連携 =================
    private void openFile(File f) {
        Intent i = new Intent(Intent.ACTION_VIEW);
        i.setDataAndType(ShareProvider.uriFor(f), Fs.mime(f.getName()));
        i.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        try {
            startActivity(i);
        } catch (ActivityNotFoundException e) {
            toast("開けるアプリがありません");
        }
    }

    private void share(ArrayList<File> files) {
        ArrayList<Uri> uris = new ArrayList<>();
        File first = null;
        for (File f : files) {
            if (f.isFile()) {
                uris.add(ShareProvider.uriFor(f));
                if (first == null) first = f;
            }
        }
        if (uris.isEmpty()) {
            toast("共有できるファイルがありません");
            return;
        }
        Intent i;
        if (uris.size() == 1) {
            i = new Intent(Intent.ACTION_SEND);
            i.putExtra(Intent.EXTRA_STREAM, uris.get(0));
            i.setType(Fs.mime(first.getName()));
        } else {
            i = new Intent(Intent.ACTION_SEND_MULTIPLE);
            i.putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris);
            i.setType("*/*");
        }
        i.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        startActivity(Intent.createChooser(i, "共有"));
    }

    // ================= 動画情報・サムネイル・フォルダ集計 (非同期) =================
    private void queue(final Item it, final int pos) {
        if (!pending.add(it.key)) return;
        final int g = gen;
        metaExec.execute(() -> {
            Meta m = null;
            if (g == gen && pos >= firstVis - 9 && pos <= lastVis + 9) m = probe(it.f, true);
            final Meta r = m;
            ui.post(() -> {
                pending.remove(it.key);
                if (r != null) {
                    vcache.put(it.key, r);
                    if (r.thumb != null) thumbs.put(it.key, r.thumb);
                    else noThumb.add(it.key);
                    r.thumb = null;
                    refresh();
                }
            });
        });
    }

    private void queueDir(final String path, final int pos) {
        if (!dirPending.add(path)) return;
        final int g = gen;
        dirExec.execute(() -> {
            long[] st = null;
            if (g == gen && (pos < 0 || (pos >= firstVis - 9 && pos <= lastVis + 9))) {
                long[] acc = new long[2];
                Fs.sizeOf(new File(path), acc, new Fs.Prog() {
                    @Override
                    public boolean cancelled() {
                        return g != gen;
                    }

                    @Override
                    public void file(String n) {
                    }
                });
                if (g == gen) st = acc;
            }
            final long[] r = st;
            ui.post(() -> {
                dirPending.remove(path);
                if (r != null) {
                    dstats.put(path, r);
                    refresh();
                }
            });
        });
    }

    private void refresh() {
        if (refreshPending) return;
        refreshPending = true;
        ui.postDelayed(() -> {
            refreshPending = false;
            adapter.notifyDataSetChanged();
            updateBar();
        }, 250);
    }

    /** 動画のメタ情報(+必要ならサムネイル)を取得 */
    private static Meta probe(File f, boolean wantThumb) {
        Meta m = new Meta();
        MediaMetadataRetriever r = new MediaMetadataRetriever();
        try {
            r.setDataSource(f.getPath());
            m.dur = parse(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION));
            int w = (int) parse(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH));
            int h = (int) parse(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT));
            if (Build.VERSION.SDK_INT >= 17) {
                long rot = parse(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION));
                if (rot == 90 || rot == 270) {
                    int t = w;
                    w = h;
                    h = t;
                }
            }
            m.w = w;
            m.h = h;
            m.ok = true;
            if (wantThumb) {
                try {
                    Bitmap bmp = r.getFrameAtTime(m.dur / 10 * 1000, MediaMetadataRetriever.OPTION_CLOSEST_SYNC);
                    if (bmp == null) bmp = r.getFrameAtTime(0);
                    if (bmp != null) m.thumb = shrink(bmp);
                } catch (Throwable ignored) {
                }
            }
        } catch (Throwable e) {
            m.ok = false;
        } finally {
            try {
                r.release();
            } catch (Throwable ignored) {
            }
        }
        return m;
    }

    private static Bitmap shrink(Bitmap src) {
        int bw = src.getWidth(), bh = src.getHeight();
        float s = Math.min((float) THUMB_W / bw, (float) THUMB_H / bh);
        if (s >= 1f) s = 1f;
        int tw = Math.max(1, Math.round(bw * s)), th = Math.max(1, Math.round(bh * s));
        Bitmap out = Bitmap.createBitmap(tw, th, Bitmap.Config.RGB_565);
        Canvas c = new Canvas(out);
        c.drawBitmap(src, new Rect(0, 0, bw, bh), new Rect(0, 0, tw, th), new Paint(Paint.FILTER_BITMAP_FLAG));
        src.recycle();
        return out;
    }

    private static long parse(String s) {
        try {
            return s == null ? 0 : Long.parseLong(s.trim());
        } catch (Exception e) {
            return 0;
        }
    }

    private static String dur(long ms) {
        long s = ms / 1000, h = s / 3600, m = (s % 3600) / 60;
        s %= 60;
        return h > 0 ? String.format(Locale.US, "%d:%02d:%02d", h, m, s)
                : String.format(Locale.US, "%02d:%02d", m, s);
    }

    static String fmtSize(long b) {
        if (b < 1024) return b + "B";
        double v = b / 1024.0;
        String[] u = {"KB", "MB", "GB", "TB"};
        int i = 0;
        while (v >= 1024 && i < 3) {
            v /= 1024;
            i++;
        }
        return String.format(Locale.US, v >= 100 ? "%.0f%s" : "%.1f%s", v, u[i]);
    }

    // ================= アダプタ =================
    private static final class H {
        View root;
        ImageView iv;
        TextView ph, n, i;
    }

    private final class Adapter extends BaseAdapter {
        @Override
        public int getCount() {
            return items.size();
        }

        @Override
        public Object getItem(int i) {
            return items.get(i);
        }

        @Override
        public long getItemId(int i) {
            return i;
        }

        @Override
        public int getViewTypeCount() {
            return 2;
        }

        @Override
        public int getItemViewType(int i) {
            return grid ? 1 : 0;
        }

        @Override
        public View getView(int pos, View v, ViewGroup parent) {
            if (v == null) v = grid ? makeGrid() : makeList();
            bind((H) v.getTag(), items.get(pos), pos);
            return v;
        }
    }

    private View makeList() {
        H h = new H();
        LinearLayout wrap = new LinearLayout(this);
        wrap.setOrientation(LinearLayout.VERTICAL);
        LinearLayout row = new LinearLayout(this);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(dp(8), dp(6), dp(8), dp(6));
        FrameLayout ic = new FrameLayout(this);
        h.iv = new ImageView(this);
        h.iv.setScaleType(ImageView.ScaleType.CENTER_CROP);
        ic.addView(h.iv, new FrameLayout.LayoutParams(-1, -1));
        h.ph = new TextView(this);
        h.ph.setGravity(Gravity.CENTER);
        h.ph.setTextSize(11);
        h.ph.setTextColor(0xFF202020);
        h.ph.setSingleLine(true);
        ic.addView(h.ph, new FrameLayout.LayoutParams(-1, -1));
        row.addView(ic, new LinearLayout.LayoutParams(dp(64), dp(48)));
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        h.n = new TextView(this);
        h.n.setTextSize(16);
        h.n.setMaxLines(2);
        h.n.setEllipsize(TextUtils.TruncateAt.MIDDLE);
        h.i = new TextView(this);
        h.i.setTextSize(12);
        h.i.setTextColor(0xFF666666);
        h.i.setMaxLines(2);
        col.addView(h.n);
        col.addView(h.i);
        LinearLayout.LayoutParams cp = new LinearLayout.LayoutParams(0, -2, 1f);
        cp.leftMargin = dp(10);
        row.addView(col, cp);
        wrap.addView(row);
        View dv = new View(this);
        dv.setBackgroundColor(0x22000000);
        wrap.addView(dv, new LinearLayout.LayoutParams(-1, 1));
        h.root = wrap;
        wrap.setTag(h);
        return wrap;
    }

    private View makeGrid() {
        H h = new H();
        FrameLayout root = new FrameLayout(this);
        root.setLayoutParams(new AbsListView.LayoutParams(-1, cellH));
        root.setPadding(dp(2), dp(2), dp(2), dp(2));
        FrameLayout inner = new FrameLayout(this);
        root.addView(inner, new FrameLayout.LayoutParams(-1, -1));
        h.iv = new ImageView(this);
        h.iv.setScaleType(ImageView.ScaleType.CENTER_CROP);
        inner.addView(h.iv, new FrameLayout.LayoutParams(-1, -1));
        h.ph = new TextView(this);
        h.ph.setGravity(Gravity.CENTER);
        h.ph.setTextSize(12);
        h.ph.setTextColor(0xFF202020);
        h.ph.setPadding(dp(4), dp(4), dp(4), dp(24));
        h.ph.setMaxLines(3);
        h.ph.setEllipsize(TextUtils.TruncateAt.MIDDLE);
        inner.addView(h.ph, new FrameLayout.LayoutParams(-1, -1));
        h.i = new TextView(this);
        h.i.setTextSize(11);
        h.i.setTextColor(0xFFFFFFFF);
        h.i.setBackgroundColor(0xAA000000);
        h.i.setPadding(dp(4), dp(2), dp(4), dp(2));
        h.i.setMaxLines(2);
        inner.addView(h.i, new FrameLayout.LayoutParams(-1, -2, Gravity.BOTTOM));
        h.root = root;
        root.setTag(h);
        return root;
    }

    private void bind(H h, Item it, int pos) {
        String path = it.f.getPath();
        boolean s = sel.contains(path);
        h.root.setBackgroundColor(s ? (grid ? 0xFF3399FF : 0x553399FF) : 0);

        Meta m = null;
        Bitmap bm = null;
        long[] st = null;
        if (it.video) {
            m = vcache.get(it.key);
            bm = thumbs.get(it.key);
            if (m == null || (bm == null && !noThumb.contains(it.key))) queue(it, pos);
        } else if (it.dir) {
            st = dstats.get(path);
            if (st == null) queueDir(path, pos);
        }
        String date = df.format(new Date(it.mod));
        String stat = st == null ? "…" : st[1] + "ファイル / " + fmtSize(st[0]);
        String vi = m == null ? "…" : (m.ok ? m.w + "x" + m.h + " | " + dur(m.dur) : "解析不可");

        if (bm != null) {
            h.iv.setImageBitmap(bm);
            h.ph.setVisibility(View.GONE);
        } else {
            h.iv.setImageDrawable(null);
            h.ph.setVisibility(View.VISIBLE);
            if (it.dir) {
                h.ph.setBackgroundColor(0xFFFFE4A8);
                h.ph.setText(grid ? it.name + "/" : "DIR");
            } else if (it.video) {
                h.ph.setBackgroundColor(0xFFB0B0B0);
                h.ph.setText("…");
            } else {
                h.ph.setBackgroundColor(0xFFE0E0E0);
                String e = ext(it.name).toUpperCase(Locale.ROOT);
                if (e.length() > 4) e = e.substring(0, 4);
                h.ph.setText(grid ? it.name : e);
            }
        }

        if (grid) {
            if (it.dir) h.i.setText(stat);
            else if (it.video) h.i.setText(vi + "\n" + fmtSize(it.size));
            else h.i.setText(fmtSize(it.size));
        } else {
            h.n.setText(it.dir ? it.name + "/" : it.name);
            h.n.setTypeface(null, it.dir ? Typeface.BOLD : Typeface.NORMAL);
            h.n.setTextColor(it.dir ? 0xFF1A5FB4 : 0xFF202020);
            String info;
            if (it.dir) info = stat + " | " + date;
            else if (it.video) info = vi + " | " + fmtSize(it.size) + " | " + date;
            else info = fmtSize(it.size) + " | " + date;
            h.i.setText(info);
        }
    }

    // ================= オプションメニュー =================
    @Override
    public boolean onCreateOptionsMenu(Menu menu) {
        menu.add(0, 1, 1, "新規作成").setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM | MenuItem.SHOW_AS_ACTION_WITH_TEXT);
        menu.add(0, 2, 2, "貼り付け");
        menu.add(0, 3, 3, "検索").setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM | MenuItem.SHOW_AS_ACTION_WITH_TEXT);
        menu.add(0, 4, 4, "並べ替え");
        menu.add(0, 5, 5, "隠しファイルを表示").setCheckable(true);
        menu.add(0, 6, 6, "ストレージ切替(その他)");
        menu.add(0, 7, 7, "SDカード書込み許可");
        menu.add(0, 8, 8, "更新");
        menu.add(0, 9, 9, "上のフォルダ");
        return true;
    }

    @Override
    public boolean onPrepareOptionsMenu(Menu menu) {
        menu.findItem(2).setEnabled(!clip.isEmpty());
        menu.findItem(5).setChecked(showHidden);
        return true;
    }

    @Override
    public boolean onOptionsItemSelected(MenuItem mi) {
        switch (mi.getItemId()) {
            case 1:
                createDialog();
                return true;
            case 2:
                paste();
                return true;
            case 3:
                prompt("検索(現在のフォルダ以下)", "", q -> {
                    if (!q.isEmpty()) search(q);
                });
                return true;
            case 4:
                sortDialog();
                return true;
            case 5:
                showHidden = !showHidden;
                sp.edit().putBoolean("hidden", showHidden).apply();
                reload();
                return true;
            case 6:
                storageDialog();
                return true;
            case 7:
                askSd();
                return true;
            case 8:
                vcache.clear();
                thumbs.evictAll();
                noThumb.clear();
                reload();
                return true;
            case 9:
                if (!goUp()) toast("これより上へは移動できません");
                return true;
        }
        return super.onOptionsItemSelected(mi);
    }

    private void sortDialog() {
        String[] ks = {"名前", "更新日", "サイズ", "種類(拡張子)", "解像度", "動画の長さ"};
        LinearLayout l = new LinearLayout(this);
        l.setOrientation(LinearLayout.VERTICAL);
        l.setPadding(dp(20), dp(8), dp(20), dp(8));
        final RadioGroup g1 = new RadioGroup(this);
        for (int i = 0; i < ks.length; i++) {
            RadioButton rb = new RadioButton(this);
            rb.setText(ks[i]);
            rb.setId(100 + i);
            g1.addView(rb);
        }
        g1.check(100 + sortBy);
        l.addView(g1);
        View dv = new View(this);
        dv.setBackgroundColor(0x33000000);
        l.addView(dv, new LinearLayout.LayoutParams(-1, 1));
        final RadioGroup g2 = new RadioGroup(this);
        RadioButton a = new RadioButton(this);
        a.setText("昇順 (小さい・古い・短い順)");
        a.setId(200);
        RadioButton d = new RadioButton(this);
        d.setText("降順 (大きい・新しい・長い順)");
        d.setId(201);
        g2.addView(a);
        g2.addView(d);
        g2.check(sortAsc ? 200 : 201);
        l.addView(g2);
        ScrollView sv = new ScrollView(this);
        sv.addView(l);
        new AlertDialog.Builder(this).setTitle("並べ替え").setView(sv)
                .setPositiveButton("OK", (dlg, w) -> {
                    sortBy = g1.getCheckedRadioButtonId() - 100;
                    sortAsc = g2.getCheckedRadioButtonId() == 200;
                    sp.edit().putInt("sort", sortBy).putBoolean("asc", sortAsc).apply();
                    sortItems();
                    lv.setSelection(0);
                    ensureSortMeta();
                }).setNegativeButton("キャンセル", null).show();
    }

    private void storageDialog() {
        final ArrayList<File> dirs = new ArrayList<>();
        ArrayList<String> names = new ArrayList<>();
        dirs.add(fs.primary);
        names.add("内部ストレージ");
        for (File r : fs.sdRoots) {
            dirs.add(r);
            names.add("SDカード (" + r.getName() + ")");
        }
        if (Build.VERSION.SDK_INT < 21) {
            for (File d : fs.sdAppDirs) {
                dirs.add(d);
                names.add("SDカード アプリ専用領域(書込可)");
            }
        }
        dirs.add(new File("/"));
        names.add("/ (ルート)");
        new AlertDialog.Builder(this).setTitle("ストレージ")
                .setItems(names.toArray(new String[0]), (d, w) -> open(dirs.get(w), false)).show();
    }

    // ================= SDカード書込み許可 =================
    private boolean ensureWritable(File dir) {
        if (dir.canWrite() || fs.docUri(dir) != null) return true;
        if (fs.rootOf(dir) != null) askSd();
        else toast("このフォルダには書き込めません");
        return false;
    }

    private void askSd() {
        if (fs.sdRoots.isEmpty()) {
            toast("SDカードが見つかりません");
            return;
        }
        if (Build.VERSION.SDK_INT < 21) {
            new AlertDialog.Builder(this).setTitle("SDカードへの書き込み")
                    .setMessage("Android 4.4 では OS の制限により、SDカード上で書き込めるのは各アプリ専用領域"
                            + "(Android/data/…)のみです。\nメニューの「ストレージ切替(その他)」→「SDカード アプリ専用領域」から操作できます。"
                            + "\n※読み取りはSDカード全体で可能です。")
                    .setPositiveButton("OK", null).show();
            return;
        }
        new AlertDialog.Builder(this).setTitle("SDカードの書込み許可")
                .setMessage("次の画面でSDカードを選び、最上位フォルダのまま「このフォルダを使用」を押してください。")
                .setPositiveButton("OK", (d, w) -> {
                    try {
                        startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), REQ_TREE);
                    } catch (ActivityNotFoundException e) {
                        toast("この端末では許可画面を開けません");
                    }
                }).setNegativeButton("キャンセル", null).show();
    }

    @Override
    protected void onActivityResult(int rq, int rc, Intent data) {
        if (rq != REQ_TREE || rc != RESULT_OK || data == null || Build.VERSION.SDK_INT < 21) return;
        Uri t = data.getData();
        if (t == null) return;
        String tid = DocumentsContract.getTreeDocumentId(t);
        if (!tid.endsWith(":") || tid.startsWith("primary")) {
            toast("SDカードの最上位フォルダを選んでください");
            return;
        }
        String vol = tid.substring(0, tid.length() - 1);
        File root = null;
        for (File r : fs.sdRoots) if (r.getName().equalsIgnoreCase(vol)) root = r;
        if (root == null && fs.sdRoots.size() == 1) root = fs.sdRoots.get(0);
        if (root == null) {
            toast("対応するSDカードを特定できませんでした");
            return;
        }
        try {
            getContentResolver().takePersistableUriPermission(t,
                    Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
        } catch (Exception ignored) {
        }
        fs.setTree(root, t);
        toast("SDカードへの書き込みを許可しました");
    }

    // ================= 操作 =================
    private void runJob(String title, final Job job, final Runnable after) {
        final AtomicBoolean cancel = new AtomicBoolean();
        final ProgressDialog pd = new ProgressDialog(this);
        pd.setTitle(title);
        pd.setMessage(" ");
        pd.setCancelable(false);
        pd.setButton(DialogInterface.BUTTON_NEGATIVE, "中止", (d, w) -> cancel.set(true));
        pd.show();
        final Fs.Prog prog = new Fs.Prog() {
            @Override
            public boolean cancelled() {
                return cancel.get();
            }

            @Override
            public void file(final String n) {
                ui.post(() -> pd.setMessage(n));
            }
        };
        new Thread(() -> {
            String e = null;
            try {
                job.run(prog);
            } catch (Exception ex) {
                e = ex.getMessage() != null ? ex.getMessage() : ex.toString();
            }
            final String err = e;
            ui.post(() -> {
                try {
                    pd.dismiss();
                } catch (Exception ignored) {
                }
                if (err != null) toast(err);
                after.run();
            });
        }).start();
    }

    private void paste() {
        if (clip.isEmpty() || cwd == null) return;
        final ArrayList<File> src = new ArrayList<>(clip);
        final boolean cut = clipCut;
        final File dst = cwd;
        if (!ensureWritable(dst)) return;
        runJob(cut ? "移動中" : "コピー中", p -> {
            for (File s : src) {
                if (s.isDirectory() && (dst.getPath().equals(s.getPath())
                        || dst.getPath().startsWith(s.getPath() + "/")))
                    throw new IOException("フォルダ自身の中には貼り付けできません");
                if (cut && dst.equals(s.getParentFile())) continue;
                File t = new File(dst, Fs.unique(dst, s.getName()));
                p.file(s.getName());
                if (cut) fs.move(s, t, p);
                else fs.copy(s, t, p);
            }
        }, () -> {
            if (cut) clip.clear();
            reload();
        });
    }

    private void confirmDelete(final ArrayList<File> files) {
        new AlertDialog.Builder(this).setTitle("削除")
                .setMessage(files.size() + " 件を削除します。元に戻せません。")
                .setPositiveButton("削除", (d, w) -> {
                    if (!ensureWritable(cwd)) return;
                    runJob("削除中", p -> {
                        int fail = 0;
                        for (File f : files) {
                            if (p.cancelled()) break;
                            p.file(f.getName());
                            if (!fs.delete(f)) fail++;
                        }
                        if (fail > 0) throw new IOException(fail + " 件を削除できませんでした");
                    }, this::reload);
                }).setNegativeButton("キャンセル", null).show();
    }

    private void prompt(String title, String init, final Ok ok) {
        final EditText e = new EditText(this);
        e.setInputType(InputType.TYPE_CLASS_TEXT);
        e.setText(init);
        e.setSelection(init.length());
        new AlertDialog.Builder(this).setTitle(title).setView(e)
                .setPositiveButton("OK", (d, w) -> ok.accept(e.getText().toString().trim()))
                .setNegativeButton("キャンセル", null).show();
    }

    private void createDialog() {
        if (cwd == null) return;
        new AlertDialog.Builder(this).setTitle("新規作成")
                .setItems(new String[]{"フォルダ", "ファイル(空)"}, (d, w) -> {
                    if (!ensureWritable(cwd)) return;
                    final boolean isDir = w == 0;
                    prompt(isDir ? "フォルダ名" : "ファイル名 (例: memo.txt)", "", n -> {
                        if (n.isEmpty() || n.contains("/")) {
                            toast("名前が不正です");
                            return;
                        }
                        File t = new File(cwd, n);
                        if (t.exists()) {
                            toast("同名のファイルまたはフォルダがあります");
                            return;
                        }
                        boolean ok = isDir ? fs.mkdir(t) : fs.createFile(t);
                        if (!ok) toast("作成できませんでした");
                        reload();
                    });
                }).show();
    }

    private void rename(final File f) {
        prompt("名前変更", f.getName(), n -> {
            if (n.isEmpty() || n.contains("/")) {
                toast("名前が不正です");
                return;
            }
            if (new File(f.getParentFile(), n).exists()) {
                toast("同名のファイルがあります");
                return;
            }
            if (!ensureWritable(f.getParentFile())) return;
            if (!fs.rename(f, n)) toast("変更できませんでした");
            reload();
        });
    }

    private void search(final String q) {
        final ArrayList<Item> res = new ArrayList<>();
        final File base = cwd;
        final String ql = q.toLowerCase();
        runJob("検索中", p -> {
            ArrayDeque<File> st = new ArrayDeque<>();
            st.push(base);
            int visited = 0;
            while (!st.isEmpty() && res.size() < 1000 && visited++ < 30000) {
                if (p.cancelled()) break;
                File d = st.pop();
                File[] c = d.listFiles();
                if (c == null) continue;
                p.file(d.getPath());
                for (File f : c) {
                    if (f.getName().toLowerCase().contains(ql)) res.add(mk(f));
                    if (f.isDirectory()) st.push(f);
                }
            }
        }, () -> {
            searching = true;
            searchQuery = q + " (" + res.size() + "件)";
            gen++;
            pending.clear();
            dirPending.clear();
            sel.clear();
            items = res;
            Collections.sort(items, cmp);
            buildPath();
            updateSelUi();
            lv.setSelection(0);
            ensureSortMeta();
        });
    }

    private void detail(final File f) {
        final long[] acc = new long[2];
        runJob("情報取得中", p -> Fs.sizeOf(f, acc, p), () -> {
            StringBuilder sb = new StringBuilder();
            sb.append("場所: ").append(f.getParent()).append("\n");
            sb.append("更新: ").append(df.format(new Date(f.lastModified()))).append("\n");
            if (f.isDirectory()) {
                sb.append("ファイル数: ").append(acc[1]).append("\n");
                sb.append("合計サイズ: ").append(fmtSize(acc[0])).append("\n");
            } else {
                sb.append("サイズ: ").append(fmtSize(f.length())).append(" (").append(f.length()).append(" B)\n");
                sb.append("種類: ").append(Fs.mime(f.getName())).append("\n");
                if (isVideo(f.getName())) {
                    Meta m = probe(f, false);
                    sb.append("動画: ").append(m.ok ? m.w + "x" + m.h + " | " + dur(m.dur) : "解析不可").append("\n");
                }
            }
            sb.append(f.canWrite() ? "書込: 可" : "書込: 不可");
            new AlertDialog.Builder(MainActivity.this).setTitle(f.getName())
                    .setMessage(sb.toString()).setPositiveButton("OK", null).show();
        });
    }

    // ================= 雑 =================
    private int dp(float v) {
        return (int) (v * getResources().getDisplayMetrics().density + 0.5f);
    }

    private void toast(String s) {
        Toast.makeText(this, s, Toast.LENGTH_LONG).show();
    }
}
