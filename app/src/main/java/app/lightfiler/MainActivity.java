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
import android.graphics.Typeface;
import android.media.MediaMetadataRetriever;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.provider.DocumentsContract;
import android.text.InputType;
import android.text.TextUtils;
import android.view.ActionMode;
import android.view.Menu;
import android.view.MenuItem;
import android.view.View;
import android.view.ViewGroup;
import android.widget.AbsListView;
import android.widget.AdapterView;
import android.widget.BaseAdapter;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ListView;
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

    // ---------- データ ----------
    static final class Item {
        File f;
        String name, key;
        boolean dir, video;
        long size, mod;
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

    private Fs fs;
    private SharedPreferences sp;
    private File cwd;
    private ArrayList<Item> items = new ArrayList<>();
    private final HashSet<String> sel = new HashSet<>();
    private final ArrayList<File> clip = new ArrayList<>();
    private boolean clipCut, showHidden, searching;
    private int sortBy;
    private ListView lv;
    private TextView pathView;
    private Adapter adapter;
    private ActionMode mode;

    private final Handler ui = new Handler(Looper.getMainLooper());
    private final ExecutorService exec = Executors.newSingleThreadExecutor();
    private final HashMap<String, String> vcache = new HashMap<>();
    private final HashSet<String> pending = new HashSet<>();
    private final HashMap<String, int[]> positions = new HashMap<>();
    private final SimpleDateFormat df = new SimpleDateFormat("yyyy/MM/dd HH:mm", Locale.getDefault());
    private volatile int gen, firstVis, lastVis = 40;
    private boolean refreshPending;

    // ---------- ライフサイクル ----------
    @Override
    protected void onCreate(Bundle b) {
        super.onCreate(b);
        fs = new Fs(this);
        sp = getSharedPreferences("p", 0);
        sortBy = sp.getInt("sort", 0);
        showHidden = sp.getBoolean("hidden", false);

        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        pathView = new TextView(this);
        pathView.setPadding(dp(10), dp(6), dp(10), dp(6));
        pathView.setBackgroundColor(0xFFE8E8E8);
        pathView.setTextSize(13);
        pathView.setSingleLine(true);
        pathView.setEllipsize(TextUtils.TruncateAt.START);
        root.addView(pathView, new LinearLayout.LayoutParams(-1, -2));

        lv = new ListView(this);
        adapter = new Adapter();
        lv.setAdapter(adapter);
        lv.setFastScrollEnabled(true);
        root.addView(lv, new LinearLayout.LayoutParams(-1, 0, 1f));
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
                if (mode != null) toggle(it);
                else if (it.dir) open(it.f, false);
                else openFile(it.f);
            }
        });
        lv.setOnItemLongClickListener(new AdapterView.OnItemLongClickListener() {
            @Override
            public boolean onItemLongClick(AdapterView<?> p, View v, int i, long id) {
                if (mode == null) mode = startActionMode(cab);
                toggle(items.get(i));
                return true;
            }
        });

        if (Build.VERSION.SDK_INT >= 23
                && checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[]{Manifest.permission.READ_EXTERNAL_STORAGE,
                    Manifest.permission.WRITE_EXTERNAL_STORAGE}, REQ_PERM);
        } else {
            start();
        }
    }

    @Override
    public void onRequestPermissionsResult(int rq, String[] perms, int[] res) {
        start();
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
        exec.shutdownNow();
    }

    @Override
    public void onBackPressed() {
        if (mode != null) {
            mode.finish();
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

    // ---------- 一覧 ----------
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

    private final Comparator<Item> cmp = new Comparator<Item>() {
        @Override
        public int compare(Item a, Item b) {
            if (a.dir != b.dir) return a.dir ? -1 : 1;
            int r;
            switch (sortBy) {
                case 1:
                    r = Long.compare(b.mod, a.mod);
                    break;
                case 2:
                    r = Long.compare(b.size, a.size);
                    break;
                case 3:
                    r = ext(a.name).compareToIgnoreCase(ext(b.name));
                    break;
                default:
                    r = 0;
            }
            return r != 0 ? r : a.name.compareToIgnoreCase(b.name);
        }
    };

    private void open(File dir, boolean restore) {
        File[] fl = dir.listFiles();
        if (fl == null) {
            toast("開けません: " + dir.getPath());
            return;
        }
        if (cwd != null && !searching) {
            View c0 = lv.getChildAt(0);
            positions.put(cwd.getPath(), new int[]{lv.getFirstVisiblePosition(), c0 == null ? 0 : c0.getTop()});
        }
        searching = false;
        gen++;
        pending.clear();
        if (mode != null) mode.finish();
        cwd = dir;
        ArrayList<Item> n = new ArrayList<>(fl.length);
        for (File f : fl) {
            if (!showHidden && f.getName().startsWith(".")) continue;
            n.add(mk(f));
        }
        Collections.sort(n, cmp);
        items = n;
        firstVis = 0;
        lastVis = 40;
        adapter.notifyDataSetChanged();
        pathView.setText(dir.getPath() + (n.isEmpty() ? "  (空)" : "  (" + n.size() + ")"));
        int[] p = restore ? positions.get(dir.getPath()) : null;
        if (p != null) lv.setSelectionFromTop(p[0], p[1]);
        else lv.setSelection(0);
    }

    private void reload() {
        open(cwd, true);
    }

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
        for (File f : files) if (f.isFile()) uris.add(ShareProvider.uriFor(f));
        if (uris.isEmpty()) {
            toast("共有できるファイルがありません");
            return;
        }
        Intent i;
        if (uris.size() == 1) {
            i = new Intent(Intent.ACTION_SEND);
            i.putExtra(Intent.EXTRA_STREAM, uris.get(0));
            i.setType(Fs.mime(files.get(0).getName()));
        } else {
            i = new Intent(Intent.ACTION_SEND_MULTIPLE);
            i.putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris);
            i.setType("*/*");
        }
        i.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        startActivity(Intent.createChooser(i, "共有"));
    }

    // ---------- 動画情報(バックグラウンド取得) ----------
    private void queue(final Item it, final int pos) {
        if (!pending.add(it.key)) return;
        final int g = gen;
        exec.execute(new Runnable() {
            @Override
            public void run() {
                String m = null;
                if (g == gen && pos >= firstVis - 8 && pos <= lastVis + 8) m = probe(it.f);
                final String r = m;
                ui.post(new Runnable() {
                    @Override
                    public void run() {
                        pending.remove(it.key);
                        if (r != null) {
                            vcache.put(it.key, r);
                            refresh();
                        }
                    }
                });
            }
        });
    }

    private void refresh() {
        if (refreshPending) return;
        refreshPending = true;
        ui.postDelayed(new Runnable() {
            @Override
            public void run() {
                refreshPending = false;
                adapter.notifyDataSetChanged();
            }
        }, 250);
    }

    private static String probe(File f) {
        MediaMetadataRetriever r = new MediaMetadataRetriever();
        try {
            r.setDataSource(f.getPath());
            String d = r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION);
            int w = parse(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH));
            int h = parse(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT));
            if (Build.VERSION.SDK_INT >= 17) {
                int rot = parse(r.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION));
                if (rot == 90 || rot == 270) {
                    int t = w;
                    w = h;
                    h = t;
                }
            }
            return dur(d == null ? 0 : parse(d)) + " | " + (w > 0 ? w + "x" + h : "?");
        } catch (Throwable e) {
            return "--:-- | ?";
        } finally {
            try {
                r.release();
            } catch (Throwable ignored) {
            }
        }
    }

    private static int parse(String s) {
        try {
            return s == null ? 0 : Integer.parseInt(s.trim());
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

    private String info(Item it, int pos) {
        String d = df.format(new Date(it.mod));
        if (it.dir) return d;
        String s = fmtSize(it.size);
        if (!it.video) return s + " | " + d;
        String m = vcache.get(it.key);
        if (m == null) {
            queue(it, pos);
            m = "…";
        }
        return m + " | " + s + " | " + d;
    }

    // ---------- アダプタ ----------
    private static final class H {
        TextView n, i;
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
        public View getView(int i, View v, ViewGroup p) {
            H h;
            if (v == null) {
                LinearLayout l = new LinearLayout(MainActivity.this);
                l.setOrientation(LinearLayout.VERTICAL);
                l.setPadding(dp(10), dp(7), dp(10), dp(7));
                h = new H();
                h.n = new TextView(MainActivity.this);
                h.n.setTextSize(16);
                h.n.setMaxLines(2);
                h.n.setEllipsize(TextUtils.TruncateAt.MIDDLE);
                h.i = new TextView(MainActivity.this);
                h.i.setTextSize(12);
                h.i.setTextColor(0xFF666666);
                h.i.setSingleLine(true);
                l.addView(h.n);
                l.addView(h.i);
                l.setTag(h);
                v = l;
            } else {
                h = (H) v.getTag();
            }
            Item it = items.get(i);
            h.n.setText(it.dir ? it.name + "/" : it.name);
            h.n.setTypeface(null, it.dir ? Typeface.BOLD : Typeface.NORMAL);
            h.n.setTextColor(it.dir ? 0xFF1A5FB4 : 0xFF202020);
            h.i.setText(info(it, i));
            v.setBackgroundColor(sel.contains(it.f.getPath()) ? 0x553399FF : 0);
            return v;
        }
    }

    // ---------- 選択モード ----------
    private void toggle(Item it) {
        String p = it.f.getPath();
        if (!sel.remove(p)) sel.add(p);
        if (sel.isEmpty()) {
            if (mode != null) mode.finish();
        } else if (mode != null) {
            mode.setTitle(sel.size() + " 件選択");
            mode.invalidate();
        }
        adapter.notifyDataSetChanged();
    }

    private ArrayList<File> selected() {
        ArrayList<File> r = new ArrayList<>();
        for (Item it : items) if (sel.contains(it.f.getPath())) r.add(it.f);
        return r;
    }

    private final ActionMode.Callback cab = new ActionMode.Callback() {
        @Override
        public boolean onCreateActionMode(ActionMode m, Menu menu) {
            add(menu, 1, "コピー");
            add(menu, 2, "切り取り");
            add(menu, 3, "削除");
            add(menu, 4, "名前変更");
            add(menu, 5, "共有");
            add(menu, 6, "全選択");
            add(menu, 7, "詳細");
            return true;
        }

        private void add(Menu menu, int id, String t) {
            menu.add(0, id, id, t).setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM);
        }

        @Override
        public boolean onPrepareActionMode(ActionMode m, Menu menu) {
            menu.findItem(4).setVisible(sel.size() == 1);
            menu.findItem(7).setVisible(sel.size() == 1);
            return true;
        }

        @Override
        public boolean onActionItemClicked(ActionMode m, MenuItem mi) {
            ArrayList<File> s = selected();
            switch (mi.getItemId()) {
                case 1:
                case 2:
                    clip.clear();
                    clip.addAll(s);
                    clipCut = mi.getItemId() == 2;
                    toast((clipCut ? "切り取り: " : "コピー: ") + s.size() + " 件 → 貼り付けは上部メニューから");
                    m.finish();
                    return true;
                case 3:
                    confirmDelete(s);
                    return true;
                case 4:
                    if (s.size() == 1) rename(s.get(0));
                    return true;
                case 5:
                    share(s);
                    return true;
                case 6:
                    for (Item it : items) sel.add(it.f.getPath());
                    m.setTitle(sel.size() + " 件選択");
                    m.invalidate();
                    adapter.notifyDataSetChanged();
                    return true;
                case 7:
                    if (s.size() == 1) detail(s.get(0));
                    return true;
            }
            return false;
        }

        @Override
        public void onDestroyActionMode(ActionMode m) {
            mode = null;
            sel.clear();
            adapter.notifyDataSetChanged();
        }
    };

    // ---------- オプションメニュー ----------
    @Override
    public boolean onCreateOptionsMenu(Menu menu) {
        menu.add(0, 1, 1, "新規フォルダ");
        menu.add(0, 2, 2, "貼り付け").setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM | MenuItem.SHOW_AS_ACTION_WITH_TEXT);
        menu.add(0, 3, 3, "検索").setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM | MenuItem.SHOW_AS_ACTION_WITH_TEXT);
        menu.add(0, 4, 4, "並べ替え");
        menu.add(0, 5, 5, "隠しファイルを表示").setCheckable(true);
        menu.add(0, 6, 6, "ストレージ切替");
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
                newFolder();
                return true;
            case 2:
                paste();
                return true;
            case 3:
                searchDialog();
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
                reload();
                return true;
            case 9:
                if (!goUp()) toast("これより上へは移動できません(ストレージ切替を使用)");
                return true;
        }
        return super.onOptionsItemSelected(mi);
    }

    private void sortDialog() {
        new AlertDialog.Builder(this).setTitle("並べ替え")
                .setSingleChoiceItems(new String[]{"名前", "更新日(新しい順)", "サイズ(大きい順)", "種類(拡張子)"}, sortBy,
                        new DialogInterface.OnClickListener() {
                            @Override
                            public void onClick(DialogInterface d, int w) {
                                sortBy = w;
                                sp.edit().putInt("sort", w).apply();
                                d.dismiss();
                                reload();
                            }
                        }).show();
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
                .setItems(names.toArray(new String[0]), new DialogInterface.OnClickListener() {
                    @Override
                    public void onClick(DialogInterface d, int w) {
                        open(dirs.get(w), false);
                    }
                }).show();
    }

    // ---------- SDカード書込み許可 ----------
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
                            + "(Android/data/…)のみです。\n「ストレージ切替」→「SDカード アプリ専用領域」から操作できます。"
                            + "\n※読み取りはSDカード全体で可能です。")
                    .setPositiveButton("OK", null).show();
            return;
        }
        new AlertDialog.Builder(this).setTitle("SDカードの書込み許可")
                .setMessage("次の画面でSDカードを選び、最上位フォルダのまま「このフォルダを使用」を押してください。")
                .setPositiveButton("OK", new DialogInterface.OnClickListener() {
                    @Override
                    public void onClick(DialogInterface d, int w) {
                        try {
                            startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), REQ_TREE);
                        } catch (ActivityNotFoundException e) {
                            toast("この端末では許可画面を開けません");
                        }
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

    // ---------- 操作 ----------
    private void runJob(String title, final Job job, final Runnable after) {
        final AtomicBoolean cancel = new AtomicBoolean();
        final ProgressDialog pd = new ProgressDialog(this);
        pd.setTitle(title);
        pd.setMessage(" ");
        pd.setCancelable(false);
        pd.setButton(DialogInterface.BUTTON_NEGATIVE, "中止", new DialogInterface.OnClickListener() {
            @Override
            public void onClick(DialogInterface d, int w) {
                cancel.set(true);
            }
        });
        pd.show();
        final Fs.Prog prog = new Fs.Prog() {
            @Override
            public boolean cancelled() {
                return cancel.get();
            }

            @Override
            public void file(final String n) {
                ui.post(new Runnable() {
                    @Override
                    public void run() {
                        pd.setMessage(n);
                    }
                });
            }
        };
        new Thread(new Runnable() {
            @Override
            public void run() {
                String e = null;
                try {
                    job.run(prog);
                } catch (Exception ex) {
                    e = ex.getMessage() != null ? ex.getMessage() : ex.toString();
                }
                final String err = e;
                ui.post(new Runnable() {
                    @Override
                    public void run() {
                        try {
                            pd.dismiss();
                        } catch (Exception ignored) {
                        }
                        if (err != null) toast(err);
                        after.run();
                    }
                });
            }
        }).start();
    }

    private void paste() {
        if (clip.isEmpty() || cwd == null) return;
        final ArrayList<File> src = new ArrayList<>(clip);
        final boolean cut = clipCut;
        final File dst = cwd;
        if (!ensureWritable(dst)) return;
        runJob(cut ? "移動中" : "コピー中", new Job() {
            @Override
            public void run(Fs.Prog p) throws Exception {
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
            }
        }, new Runnable() {
            @Override
            public void run() {
                if (cut) clip.clear();
                reload();
            }
        });
    }

    private void confirmDelete(final ArrayList<File> files) {
        new AlertDialog.Builder(this).setTitle("削除")
                .setMessage(files.size() + " 件を削除します。元に戻せません。")
                .setPositiveButton("削除", new DialogInterface.OnClickListener() {
                    @Override
                    public void onClick(DialogInterface d, int w) {
                        if (mode != null) mode.finish();
                        if (!ensureWritable(cwd)) return;
                        runJob("削除中", new Job() {
                            @Override
                            public void run(Fs.Prog p) throws Exception {
                                int fail = 0;
                                for (File f : files) {
                                    if (p.cancelled()) break;
                                    p.file(f.getName());
                                    if (!fs.delete(f)) fail++;
                                }
                                if (fail > 0) throw new IOException(fail + " 件を削除できませんでした");
                            }
                        }, new Runnable() {
                            @Override
                            public void run() {
                                reload();
                            }
                        });
                    }
                }).setNegativeButton("キャンセル", null).show();
    }

    private void prompt(String title, String init, final Ok ok) {
        final EditText e = new EditText(this);
        e.setInputType(InputType.TYPE_CLASS_TEXT);
        e.setText(init);
        e.setSelection(init.length());
        new AlertDialog.Builder(this).setTitle(title).setView(e)
                .setPositiveButton("OK", new DialogInterface.OnClickListener() {
                    @Override
                    public void onClick(DialogInterface d, int w) {
                        ok.accept(e.getText().toString().trim());
                    }
                }).setNegativeButton("キャンセル", null).show();
    }

    private void newFolder() {
        if (cwd == null || !ensureWritable(cwd)) return;
        prompt("新規フォルダ", "", new Ok() {
            @Override
            public void accept(String n) {
                if (n.isEmpty() || n.contains("/")) {
                    toast("名前が不正です");
                    return;
                }
                if (!fs.mkdir(new File(cwd, n))) toast("作成できませんでした");
                reload();
            }
        });
    }

    private void rename(final File f) {
        prompt("名前変更", f.getName(), new Ok() {
            @Override
            public void accept(String n) {
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
                if (mode != null) mode.finish();
                reload();
            }
        });
    }

    private void searchDialog() {
        prompt("検索(現在のフォルダ以下)", "", new Ok() {
            @Override
            public void accept(String q) {
                if (!q.isEmpty()) search(q);
            }
        });
    }

    private void search(final String q) {
        final ArrayList<Item> res = new ArrayList<>();
        final File base = cwd;
        final String ql = q.toLowerCase();
        runJob("検索中", new Job() {
            @Override
            public void run(Fs.Prog p) {
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
            }
        }, new Runnable() {
            @Override
            public void run() {
                Collections.sort(res, cmp);
                searching = true;
                gen++;
                pending.clear();
                items = res;
                adapter.notifyDataSetChanged();
                lv.setSelection(0);
                pathView.setText("検索: " + q + " (" + res.size() + "件)  ※戻るキーで元のフォルダへ");
            }
        });
    }

    private void detail(final File f) {
        final long[] acc = new long[2];
        runJob("情報取得中", new Job() {
            @Override
            public void run(Fs.Prog p) {
                Fs.sizeOf(f, acc, p);
            }
        }, new Runnable() {
            @Override
            public void run() {
                StringBuilder sb = new StringBuilder();
                sb.append("場所: ").append(f.getParent()).append("\n");
                sb.append("更新: ").append(df.format(new Date(f.lastModified()))).append("\n");
                if (f.isDirectory()) {
                    sb.append("ファイル数: ").append(acc[1]).append("\n");
                    sb.append("合計サイズ: ").append(fmtSize(acc[0])).append("\n");
                } else {
                    sb.append("サイズ: ").append(fmtSize(f.length())).append(" (").append(f.length()).append(" B)\n");
                    sb.append("種類: ").append(Fs.mime(f.getName())).append("\n");
                    if (isVideo(f.getName())) sb.append("動画: ").append(probe(f)).append("\n");
                }
                sb.append(f.canWrite() ? "書込: 可" : "書込: 不可");
                new AlertDialog.Builder(MainActivity.this).setTitle(f.getName())
                        .setMessage(sb.toString()).setPositiveButton("OK", null).show();
            }
        });
    }

    // ---------- 雑 ----------
    private int dp(float v) {
        return (int) (v * getResources().getDisplayMetrics().density + 0.5f);
    }

    private void toast(String s) {
        Toast.makeText(this, s, Toast.LENGTH_LONG).show();
    }
}
