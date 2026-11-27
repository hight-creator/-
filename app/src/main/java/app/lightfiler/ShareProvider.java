package app.lightfiler;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;

import java.io.File;
import java.io.FileNotFoundException;

/** 他アプリへ一時的にファイルを渡すための読み取り専用プロバイダ(外部公開なし・URI権限付与のみ)。 */
public class ShareProvider extends ContentProvider {
    static final String AUTH = "app.lightfiler.share";

    static Uri uriFor(File f) {
        return new Uri.Builder().scheme("content").authority(AUTH)
                .encodedPath(Uri.encode(f.getAbsolutePath(), "/")).build();
    }

    private static File fileOf(Uri u) {
        return new File(u.getPath());
    }

    @Override
    public boolean onCreate() {
        return true;
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        return ParcelFileDescriptor.open(fileOf(uri), ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override
    public Cursor query(Uri uri, String[] proj, String sel, String[] args, String sort) {
        File f = fileOf(uri);
        String[] cols = {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE};
        MatrixCursor c = new MatrixCursor(cols);
        c.addRow(new Object[]{f.getName(), f.length()});
        return c;
    }

    @Override
    public String getType(Uri uri) {
        return Fs.mime(fileOf(uri).getName());
    }

    @Override
    public Uri insert(Uri uri, ContentValues v) {
        throw new UnsupportedOperationException();
    }

    @Override
    public int delete(Uri uri, String s, String[] a) {
        return 0;
    }

    @Override
    public int update(Uri uri, ContentValues v, String s, String[] a) {
        return 0;
    }
}
