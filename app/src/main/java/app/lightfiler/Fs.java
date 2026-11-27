package app.lightfiler;

import android.content.ContentResolver;
import android.content.Context;
import android.content.SharedPreferences;
import android.net.Uri;
import android.os.Build;
import android.os.Environment;
import android.provider.DocumentsContract;
import android.webkit.MimeTypeMap;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.Locale;

/**
 * ファイル操作。通常は File API を使い、SDカードで失敗した場合は
 * Android 5.0+ の Storage Access Framework (DocumentsContract) に切り替える。
 */
final class Fs {

    interface Prog {
        boolean cancelled();
        void file(String name);
    }

    private final Context ctx;
    private final SharedPreferences sp;
    final File primary;
    final ArrayList<File> sdRoots = new ArrayList<>();
    /** SD上のアプリ専用領域(Android 4.4 でも書き込み可能) */
    final ArrayList<File> sdAppDirs = new ArrayList<>();

    Fs(Context c) {
        ctx = c;
        sp = c.getSharedPreferences("p", 0);
        primary = Environment.getExternalStorageDirectory();
        scan();
    }

    private void scan() {
        sdRoots.clear();
        sdAppDirs.clear();
        File[] dirs = Build.VERSION.SDK_INT >= 19 ? ctx.getExternalFilesDirs(null) : null;
        if (dirs == null) dirs = new File[0];
        for (File d : dirs) {
            if (d == null) continue;
            String p = d.getAbsolutePath();
            int i = p.indexOf("/Android/data");
            if (i <= 0) continue;
            File root = new File(p.substring(0, i));
            if (root.equals(primary) || sdRoots.contains(root)) continue;
            sdRoots.add(root);
            sdAppDirs.add(d);
        }
        // 上で見つからない端末向け: /storage 直下を走査
        if (sdRoots.isEmpty()) {
            File[] st = new File("/storage").listFiles();
            if (st != null) {
                String pc;
                try {
                    pc = primary.getCanonicalPath();
                } catch (IOException e) {
                    pc = primary.getPath();
                }
                for (File s : st) {
                    String n = s.getName();
                    if (n.equals("emulated") || n.equals("self") || !s.isDirectory() || !s.canRead()) continue;
                    try {
                        if (s.getCanonicalPath().equals(pc)) continue;
                    } catch (IOException e) {
                        continue;
                    }
                    if (s.equals(primary)) continue;
                    sdRoots.add(s);
                }
            }
        }
    }

    File rootOf(File f) {
        String p = f.getPath();
        for (File r : sdRoots) {
            String rp = r.getPath();
            if (p.equals(rp) || p.startsWith(rp + "/")) return r;
        }
        return null;
    }

    void setTree(File root, Uri tree) {
        sp.edit().putString("tree_" + root.getPath(), tree.toString()).apply();
    }

    /** SAF 上の対応する Document Uri。許可未取得なら null。 */
    Uri docUri(File f) {
        if (Build.VERSION.SDK_INT < 21) return null;
        File root = rootOf(f);
        if (root == null) return null;
        String ts = sp.getString("tree_" + root.getPath(), null);
        if (ts == null) return null;
        try {
            Uri tree = Uri.parse(ts);
            String tid = DocumentsContract.getTreeDocumentId(tree);
            int c = tid.indexOf(':');
            if (c < 0) return null;
            String vol = tid.substring(0, c);
            String rp = root.getPath();
            String rel = f.getPath().length() > rp.length() ? f.getPath().substring(rp.length() + 1) : "";
            return DocumentsContract.buildDocumentUriUsingTree(tree, vol + ":" + rel);
        } catch (Exception e) {
            return null;
        }
    }

    // ---------- 基本操作 ----------

    boolean delete(File f) {
        if (f.isDirectory()) {
            File[] c = f.listFiles();
            if (c != null) for (File x : c) delete(x);
        }
        if (f.delete()) return true;
        Uri u = docUri(f);
        if (u != null) {
            try {
                return DocumentsContract.deleteDocument(ctx.getContentResolver(), u);
            } catch (Exception ignored) {
            }
        }
        return !f.exists();
    }

    boolean mkdir(File dir) {
        if (dir.exists()) return dir.isDirectory();
        if (dir.mkdir()) return true;
        Uri pu = docUri(dir.getParentFile());
        if (pu != null) {
            try {
                return DocumentsContract.createDocument(ctx.getContentResolver(), pu,
                        DocumentsContract.Document.MIME_TYPE_DIR, dir.getName()) != null;
            } catch (Exception ignored) {
            }
        }
        return false;
    }

    boolean createFile(File f) {
        if (f.exists()) return false;
        try {
            if (f.createNewFile()) return true;
        } catch (IOException ignored) {
        }
        Uri pu = docUri(f.getParentFile());
        if (pu != null) {
            try {
                return DocumentsContract.createDocument(ctx.getContentResolver(), pu,
                        mime(f.getName()), f.getName()) != null;
            } catch (Exception ignored) {
            }
        }
        return false;
    }

    boolean rename(File f, String name) {
        File t = new File(f.getParentFile(), name);
        if (f.renameTo(t)) return true;
        Uri u = docUri(f);
        if (u != null) {
            try {
                return DocumentsContract.renameDocument(ctx.getContentResolver(), u, name) != null;
            } catch (Exception ignored) {
            }
        }
        return false;
    }

    private OutputStream openOut(File dst) throws IOException {
        try {
            return new FileOutputStream(dst);
        } catch (IOException ignored) {
        }
        Uri pu = docUri(dst.getParentFile());
        if (pu != null) {
            ContentResolver cr = ctx.getContentResolver();
            Uri n = DocumentsContract.createDocument(cr, pu, mime(dst.getName()), dst.getName());
            if (n != null) {
                OutputStream o = cr.openOutputStream(n);
                if (o != null) return o;
            }
        }
        throw new IOException("書き込めません: " + dst.getName());
    }

    void copy(File src, File dst, Prog p) throws IOException {
        if (p.cancelled()) throw new IOException("中止しました");
        if (src.isDirectory()) {
            if (!mkdir(dst)) throw new IOException("フォルダを作成できません: " + dst.getName());
            File[] c = src.listFiles();
            if (c != null) for (File x : c) copy(x, new File(dst, x.getName()), p);
            return;
        }
        p.file(src.getName());
        InputStream in = new FileInputStream(src);
        try {
            OutputStream out = openOut(dst);
            try {
                byte[] buf = new byte[128 * 1024];
                int n;
                while ((n = in.read(buf)) > 0) {
                    if (p.cancelled()) throw new IOException("中止しました");
                    out.write(buf, 0, n);
                }
            } finally {
                out.close();
            }
        } finally {
            in.close();
        }
        try {
            dst.setLastModified(src.lastModified());
        } catch (Exception ignored) {
        }
    }

    void move(File src, File dst, Prog p) throws IOException {
        if (src.renameTo(dst)) return;
        copy(src, dst, p);
        if (!delete(src)) throw new IOException("元ファイルを削除できません: " + src.getName());
    }

    // ---------- ユーティリティ ----------

    static String unique(File dir, String name) {
        if (!new File(dir, name).exists()) return name;
        int dot = name.lastIndexOf('.');
        String base = dot > 0 ? name.substring(0, dot) : name;
        String ext = dot > 0 ? name.substring(dot) : "";
        for (int i = 1; ; i++) {
            String n = base + " (" + i + ")" + ext;
            if (!new File(dir, n).exists()) return n;
        }
    }

    static String mime(String name) {
        int dot = name.lastIndexOf('.');
        if (dot >= 0) {
            String m = MimeTypeMap.getSingleton()
                    .getMimeTypeFromExtension(name.substring(dot + 1).toLowerCase(Locale.ROOT));
            if (m != null) return m;
        }
        return "application/octet-stream";
    }

    static void sizeOf(File f, long[] acc, Prog p) {
        sizeOf(f, acc, p, 0);
    }

    private static void sizeOf(File f, long[] acc, Prog p, int depth) {
        if (p.cancelled() || depth > 40) return;
        if (f.isDirectory()) {
            File[] c = f.listFiles();
            if (c != null) for (File x : c) sizeOf(x, acc, p, depth + 1);
        } else {
            acc[0] += f.length();
            acc[1]++;
        }
    }
}
