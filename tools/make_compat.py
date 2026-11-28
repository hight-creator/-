#!/usr/bin/env python3
"""
古いWindows 10 / グラフィックドライバ無しのPCでも起動できるようにする後処理。

1) vulkan-1.dll の代替(空のDLL)を作る
2) bnps.dll(互換シム)を作る
     - exe と libmpv-2.dll が使うシステムAPIを全部いったん受け取り、
       実行時に本物のDLLから探して中継する
     - 古いWindowsに存在しないAPI(AdjustWindowRectExForDpi など)は代替実装を使う
     - それでも見つからないAPIは「何もせず0を返す」関数にする(起動不能を防ぐ)
3) exe と libmpv-2.dll のインポート表を書き換えて、システムDLL 参照を bnps.dll に向ける
4) 書き換え結果を検証する
"""
import argparse
import os
import shutil
import subprocess
import sys

import pefile

SHIM = "bnps.dll"

# 書き換え対象にしないDLL(常に存在する / 別途対処済み)
SKIP_EXACT = {
    "ntdll.dll", "msvcrt.dll", "ucrtbase.dll", "vulkan-1.dll",
    "d3d11.dll", "dxgi.dll", "d3d9.dll", "d3d12.dll", "d3dcompiler_47.dll", "dcomp.dll",
}


def skip(dll):
    d = dll.lower()
    return (
        d in SKIP_EXACT
        or d == SHIM
        or d.startswith("libmpv")
        or d.startswith("api-ms-win-crt")
    )


# シムが静的に参照して先に読み込ませておくDLL: DLL名 -> (リンクするlib, 全Windowsに存在する関数)
KEEP = {
    "advapi32.dll": ("advapi32", "RegCloseKey"),
    "shell32.dll": ("shell32", "ShellExecuteW"),
    "ole32.dll": ("ole32", "CoInitialize"),
    "oleaut32.dll": ("oleaut32", "SysFreeString"),
    "winmm.dll": ("winmm", "timeBeginPeriod"),
    "imm32.dll": ("imm32", "ImmGetContext"),
    "comdlg32.dll": ("comdlg32", "GetOpenFileNameW"),
    "ws2_32.dll": ("ws2_32", "WSAStartup"),
    "dwmapi.dll": ("dwmapi", "DwmIsCompositionEnabled"),
}


def run(cmd, **kw):
    print("+", " ".join(cmd))
    subprocess.run(cmd, check=True, **kw)


def scan(pe):
    """[(entry, dll名, 関数名リスト, 序数インポートがあるか)]"""
    res = []
    for e in getattr(pe, "DIRECTORY_ENTRY_IMPORT", []):
        dll = e.dll.decode()
        names, ordinal = [], False
        for f in e.imports:
            if f.name is None:
                ordinal = True
            else:
                names.append(f.name.decode())
        res.append((e, dll, names, ordinal))
    return res


def show_imports(label, path):
    pe = pefile.PE(path, fast_load=False)
    print("== %s ==" % label)
    for _, dll, names, ordinal in scan(pe):
        print("  %-40s %4d funcs%s" % (dll, len(names), "  (序数インポートあり)" if ordinal else ""))
    for d in getattr(pe, "DIRECTORY_ENTRY_DELAY_IMPORT", []):
        print("  [delay] %s" % d.dll.decode())
    pe.close()


def make_vulkan_stub(libmpv, outdir, work):
    pe = pefile.PE(libmpv)
    names = []
    for _, dll, ns, _o in scan(pe):
        if dll.lower() == "vulkan-1.dll":
            names = ns
    pe.close()
    print("vulkan-1.dll から使われる関数:", len(names))
    src = os.path.join(work, "vulkan_stub.c")
    with open(src, "w") as f:
        for n in names:
            f.write("__declspec(dllexport) void* %s(void){return 0;}\n" % n)
        f.write("int __attribute__((unused)) bnp_vk_dummy;\n")
    run(["gcc", "-shared", "-O1", "-s", "-o", os.path.join(outdir, "vulkan-1.dll"), src])


FALLBACKS = r"""
#include <windows.h>
#include <string.h>

static HMODULE mod(const char *n) {
    HMODULE m = GetModuleHandleA(n);
    if (!m) m = LoadLibraryA(n);
    return m;
}
static UINT sysdpi(void) {
    HDC dc = GetDC(NULL);
    int d = dc ? GetDeviceCaps(dc, LOGPIXELSX) : 96;
    if (dc) ReleaseDC(NULL, dc);
    return d > 0 ? (UINT)d : 96;
}
#define CTX_SYSTEM ((void*)(INT_PTR)-2)

static BOOL WINAPI fb_AdjustWindowRectExForDpi(LPRECT r, DWORD s, BOOL m, DWORD e, UINT dpi) { (void)dpi; return AdjustWindowRectEx(r, s, m, e); }
static UINT WINAPI fb_GetDpiForWindow(HWND h) { (void)h; return sysdpi(); }
static UINT WINAPI fb_GetDpiForSystem(void) { return sysdpi(); }
static int  WINAPI fb_GetSystemMetricsForDpi(int i, UINT d) { (void)d; return GetSystemMetrics(i); }
static BOOL WINAPI fb_SystemParametersInfoForDpi(UINT a, UINT b, PVOID c, UINT d, UINT dpi) { (void)dpi; return SystemParametersInfoW(a, b, c, d); }
static void* WINAPI fb_SetThreadDpiAwarenessContext(void *c) { (void)c; return CTX_SYSTEM; }
static void* WINAPI fb_GetThreadDpiAwarenessContext(void) { return CTX_SYSTEM; }
static void* WINAPI fb_GetWindowDpiAwarenessContext(HWND h) { (void)h; return CTX_SYSTEM; }
static int   WINAPI fb_GetAwarenessFromDpiAwarenessContext(void *c) { (void)c; return 1; }
static BOOL  WINAPI fb_AreDpiAwarenessContextsEqual(void *a, void *b) { return a == b; }
static BOOL  WINAPI fb_IsValidDpiAwarenessContext(void *c) { (void)c; return TRUE; }
static BOOL  WINAPI fb_EnableNonClientDpiScaling(HWND h) { (void)h; return FALSE; }
static BOOL  WINAPI fb_SetProcessDpiAwarenessContext(void *c) { (void)c; return FALSE; }
static HRESULT WINAPI fb_GetDpiForMonitor(void *m, int t, UINT *x, UINT *y) { (void)m; (void)t; if (x) *x = 96; if (y) *y = 96; return 0; }
static HRESULT WINAPI fb_SetThreadDescription(HANDLE h, PCWSTR d) { (void)h; (void)d; return 0; }
static HRESULT WINAPI fb_GetThreadDescription(HANDLE h, PWSTR *o) { (void)h; if (o) *o = NULL; return (HRESULT)0x80004001; }
static void WINAPI fb_GetSystemTimePreciseAsFileTime(LPFILETIME t) { GetSystemTimeAsFileTime(t); }
static BOOL WINAPI fb_ProcessPrng(PBYTE p, SIZE_T n) {
    typedef BOOLEAN (WINAPI *rtlgen_t)(PVOID, ULONG);
    HMODULE a = mod("advapi32.dll");
    rtlgen_t f = a ? (rtlgen_t)GetProcAddress(a, "SystemFunction036") : NULL;
    if (f) f(p, (ULONG)n);
    return TRUE;
}
static INT_PTR WINAPI stub_zero(void) { return 0; }

struct fbent { const char *name; void *fn; };
static const struct fbent fbs[] = {
    {"AdjustWindowRectExForDpi", (void*)fb_AdjustWindowRectExForDpi},
    {"GetDpiForWindow", (void*)fb_GetDpiForWindow},
    {"GetDpiForSystem", (void*)fb_GetDpiForSystem},
    {"GetSystemMetricsForDpi", (void*)fb_GetSystemMetricsForDpi},
    {"SystemParametersInfoForDpi", (void*)fb_SystemParametersInfoForDpi},
    {"SetThreadDpiAwarenessContext", (void*)fb_SetThreadDpiAwarenessContext},
    {"GetThreadDpiAwarenessContext", (void*)fb_GetThreadDpiAwarenessContext},
    {"GetWindowDpiAwarenessContext", (void*)fb_GetWindowDpiAwarenessContext},
    {"GetAwarenessFromDpiAwarenessContext", (void*)fb_GetAwarenessFromDpiAwarenessContext},
    {"AreDpiAwarenessContextsEqual", (void*)fb_AreDpiAwarenessContextsEqual},
    {"IsValidDpiAwarenessContext", (void*)fb_IsValidDpiAwarenessContext},
    {"EnableNonClientDpiScaling", (void*)fb_EnableNonClientDpiScaling},
    {"SetProcessDpiAwarenessContext", (void*)fb_SetProcessDpiAwarenessContext},
    {"GetDpiForMonitor", (void*)fb_GetDpiForMonitor},
    {"SetThreadDescription", (void*)fb_SetThreadDescription},
    {"GetThreadDescription", (void*)fb_GetThreadDescription},
    {"GetSystemTimePreciseAsFileTime", (void*)fb_GetSystemTimePreciseAsFileTime},
    {"ProcessPrng", (void*)fb_ProcessPrng},
    {NULL, NULL}
};

static void *resolve(const char *name, const char *org) {
    static const char *alts[] = {"kernelbase.dll", "kernel32.dll", "user32.dll", "advapi32.dll",
                                 "shell32.dll", "ole32.dll", "shcore.dll", "gdi32.dll", NULL};
    HMODULE m = mod(org);
    FARPROC p = m ? GetProcAddress(m, name) : NULL;
    if (p) return (void*)p;
    for (int i = 0; alts[i]; i++) {
        HMODULE a = GetModuleHandleA(alts[i]);
        p = a ? GetProcAddress(a, name) : NULL;
        if (p) return (void*)p;
    }
    for (int i = 0; fbs[i].name; i++)
        if (strcmp(fbs[i].name, name) == 0) return fbs[i].fn;
    return (void*)stub_zero;   /* 古いWindowsに無いAPI: 何もせず0を返す */
}

BOOL WINAPI DllMain(HINSTANCE h, DWORD reason, LPVOID rsv) {
    (void)rsv;
    if (reason == DLL_PROCESS_ATTACH) {
        DisableThreadLibraryCalls(h);
        for (int i = 0; i < BNP_N; i++) bnp_tbl[i] = resolve(bnp_nm[i], bnp_org[i]);
    }
    return TRUE;
}
"""


def make_shim(entries, outdir, work):
    """entries: [(関数名, 元DLL名)]"""
    n = len(entries)
    src = os.path.join(work, "shim.c")
    with open(src, "w") as f:
        f.write("#define BNP_N %d\n" % n)
        f.write("void *bnp_tbl[BNP_N];\n")
        f.write("static const char *bnp_nm[BNP_N] = {\n")
        for name, _ in entries:
            f.write('  "%s",\n' % name)
        f.write("};\nstatic const char *bnp_org[BNP_N] = {\n")
        for _, org in entries:
            f.write('  "%s",\n' % org)
        f.write("};\n")
        f.write(FALLBACKS)
        # 各関数への中継(jmp)。実体の関数名は .def で公開名に割り当てる
        f.write('__asm__(\n".text\\n"\n')
        for i in range(n):
            f.write('".globl bnp_t%d\\n" "bnp_t%d: jmp *bnp_tbl+%d(%%rip)\\n"\n' % (i, i, 8 * i))
        f.write(");\n")

    defp = os.path.join(work, "shim.def")
    with open(defp, "w") as f:
        f.write("LIBRARY %s\nEXPORTS\n" % SHIM)
        for i, (name, _) in enumerate(entries):
            f.write("  %s=bnp_t%d\n" % (name, i))

    used = {org.lower() for _, org in entries}
    keep_src = os.path.join(work, "keep.c")
    libs, syms = [], []
    for dll, (lib, sym) in KEEP.items():
        if dll in used:
            libs.append("-l" + lib)
            syms.append(sym)
    with open(keep_src, "w") as f:
        for s in syms:
            f.write("extern void %s(void);\n" % s)
        f.write("__attribute__((used)) void *bnp_keep[] = {\n")
        for s in syms:
            f.write("  (void*)%s,\n" % s)
        f.write("  0 };\n")

    run(["gcc", "-shared", "-O2", "-s", "-o", os.path.join(outdir, SHIM),
         src, keep_src, defp, "-static-libgcc", "-luser32", "-lgdi32"] + libs)


def patch(path_in, path_out):
    pe = pefile.PE(path_in, fast_load=False)
    count = 0
    for e, dll, names, ordinal in scan(pe):
        if skip(dll):
            continue
        if ordinal:
            print("  [警告] %s は序数インポートがあるため書き換えません" % dll)
            continue
        rva = e.struct.Name
        old = pe.get_string_at_rva(rva)
        if len(old) < len(SHIM):
            print("  [警告] %s は名前が短すぎるため書き換えません" % dll)
            continue
        pe.set_bytes_at_rva(rva, SHIM.encode() + b"\0" * (len(old) - len(SHIM) + 1))
        count += 1
    # バインド済みインポート情報は無効化(DLL名を変えるため)
    pe.OPTIONAL_HEADER.DATA_DIRECTORY[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_BOUND_IMPORT"]].VirtualAddress = 0
    pe.OPTIONAL_HEADER.DATA_DIRECTORY[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_BOUND_IMPORT"]].Size = 0
    pe.OPTIONAL_HEADER.CheckSum = 0
    pe.write(filename=path_out)
    pe.close()
    print("  %s: %d 個のDLL参照を %s に変更" % (os.path.basename(path_in), count, SHIM))


def verify(shim_path, files):
    s = pefile.PE(shim_path)
    exports = {x.name.decode() for x in s.DIRECTORY_ENTRY_EXPORT.symbols if x.name}
    s.close()
    bad = 0
    for p in files:
        pe = pefile.PE(p, fast_load=False)
        for _, dll, names, _o in scan(pe):
            if dll.lower() == SHIM:
                miss = [n for n in names if n not in exports]
                if miss:
                    bad += 1
                    print("[検証NG] %s: シムに無い関数 %s" % (os.path.basename(p), miss[:10]))
        pe.close()
    if bad:
        sys.exit("検証に失敗しました")
    print("検証OK: 書き換え後のインポートは全てシムが提供しています(%d 関数をエクスポート)" % len(exports))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--dll", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()

    out = os.path.abspath(a.out)
    plain = os.path.join(out, "unpatched")
    work = os.path.join(out, "_work")
    os.makedirs(plain, exist_ok=True)
    os.makedirs(work, exist_ok=True)

    show_imports("exe (元)", a.exe)
    show_imports("libmpv-2.dll (元)", a.dll)

    # 1) vulkan-1.dll 代替
    make_vulkan_stub(a.dll, out, work)

    # 2) 無加工版(予備)
    shutil.copy(a.exe, os.path.join(plain, "blocknoise-player.exe"))
    shutil.copy(a.dll, os.path.join(plain, "libmpv-2.dll"))
    shutil.copy(os.path.join(out, "vulkan-1.dll"), os.path.join(plain, "vulkan-1.dll"))

    # 3) シム作成
    entries, seen = [], set()
    for path in (a.exe, a.dll):
        pe = pefile.PE(path, fast_load=False)
        for _, dll, names, ordinal in scan(pe):
            if skip(dll) or ordinal:
                continue
            for n in names:
                if n not in seen:
                    seen.add(n)
                    entries.append((n, dll))
        pe.close()
    print("シムが中継する関数: %d" % len(entries))
    make_shim(entries, out, work)

    # 4) 書き換え
    patch(a.exe, os.path.join(out, "blocknoise-player.exe"))
    patch(a.dll, os.path.join(out, "libmpv-2.dll"))

    # 5) 検証
    verify(os.path.join(out, SHIM),
           [os.path.join(out, "blocknoise-player.exe"), os.path.join(out, "libmpv-2.dll")])
    show_imports("exe (書き換え後)", os.path.join(out, "blocknoise-player.exe"))
    show_imports("libmpv-2.dll (書き換え後)", os.path.join(out, "libmpv-2.dll"))

    shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
