// String: a music player for your own song files, in a window of its own.
// A WPF window holding WebView2; the player is the web page in wwwroot\, served at https://string.example/.
// The page talks to this host through window.belt (wwwroot\host.js): load/save, files and folders, pickers, dropped files, programs (the Discord helper),
// and /_media/<path>, which plays a song from any folder with Range support. /_look/<file> is the person's own look:
// look.css (and pictures and fonts beside it) from <data>\look\, loaded after String's own styles; updates never touch it.
// Add-ons (extra features a person installs) live in <data>\addons\ and are served at /_addon/<id>/<file>.
// Data: %LOCALAPPDATA%\String\ (state.json, listening.json, library.json, presence.json), one dated copy per day of
// state and listening in Documents\String Backup\.
// --smoke-test --data <dir> --out <dir> [--test-media <dir>] [--timeout <s>]: off-screen, runs the page's smokeTest().
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Net.Http;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Shell;
using System.Windows.Threading;
using Microsoft.Data.Sqlite;
using Microsoft.Web.WebView2.Core;
using Microsoft.Web.WebView2.Wpf;
using Microsoft.Win32;

namespace StringApp;

public static class Program
{
    [STAThread]
    public static int Main(string[] args)
    {
        if (args.Contains("--quit"))   // updater step: ask the running copy to save and close
        {
            try { using var q = EventWaitHandle.OpenExisting("StringApp.Quit"); q.Set(); } catch { }
            return 0;
        }
        bool smoke = args.Contains("--smoke-test");
        CleanUpdateLeftovers();
        using var mutex = new Mutex(true, smoke ? $"StringApp.Smoke.{Environment.ProcessId}" : "StringApp.SingleInstance", out bool first);
        using var showSignal = new EventWaitHandle(false, EventResetMode.AutoReset, smoke ? null : "StringApp.Show");
        if (!first) { showSignal.Set(); return 0; }   // already open: bring that window forward instead

        var app = new Application { ShutdownMode = ShutdownMode.OnMainWindowClose };
        MainWindow? w = null;
        // an unexpected error keeps String running, but goes into <data>\errors.log so it can be looked into
        app.DispatcherUnhandledException += (_, e) => { e.Handled = true; w?.LogError("ui", e.Exception); };
        TaskScheduler.UnobservedTaskException += (_, e) => { e.SetObserved(); w?.LogError("task", e.Exception); };
        AppDomain.CurrentDomain.UnhandledException += (_, e) => w?.LogError("crash", e.ExceptionObject as Exception);
        var win = w = new MainWindow(args);
        app.SessionEnding += (_, _) => win.CloseNow();   // never block a Windows shutdown
        using var quitSignal = new EventWaitHandle(false, EventResetMode.AutoReset, smoke ? null : "StringApp.Quit");
        if (!smoke)
        {
            new Thread(() => { while (showSignal.WaitOne()) win.Dispatcher.BeginInvoke(new Action(win.BringToFront)); }) { IsBackground = true }.Start();
            // every request counts: one turned down (changes not saved yet) doesn't stop the next one being heard
            new Thread(() => { while (quitSignal.WaitOne()) win.Dispatcher.BeginInvoke(new Action(win.Close)); }) { IsBackground = true }.Start();
        }
        return app.Run(win);
    }

    // Files an update renamed to *.old because this copy had them open: install-update.ps1 lists them in
    // update-old.txt, and they go now that they're free. Only listed files inside String's own folder, ending in .old,
    // are deleted (never anything else that happens to end in .old), and nothing here may stop String starting.
    static void CleanUpdateLeftovers()
    {
        try
        {
            var root = Path.GetFullPath(AppContext.BaseDirectory);
            if (!root.EndsWith(Path.DirectorySeparatorChar)) root += Path.DirectorySeparatorChar;
            var list = Path.Combine(root, "update-old.txt");
            if (!File.Exists(list)) return;
            var left = new List<string>();
            foreach (var line in File.ReadAllLines(list))
            {
                var rel = line.Trim();
                if (rel.Length == 0) continue;
                string full;
                try { full = Path.GetFullPath(Path.Combine(root, rel)); } catch { continue; }
                if (!full.StartsWith(root, StringComparison.OrdinalIgnoreCase) || !full.EndsWith(".old", StringComparison.OrdinalIgnoreCase)) continue;
                try { if (File.Exists(full)) File.Delete(full); } catch { left.Add(rel); }
            }
            if (left.Count == 0) File.Delete(list); else File.WriteAllLines(list, left);
        }
        catch { }
    }
}

public sealed partial class MainWindow : Window
{
    const string Domain = "string.example";
    const string Origin = "https://" + Domain + "/";
    const string AppName = "String";

    readonly WebView2 web = new();
    readonly string dataDir, webRoot, placementPath, backupDir;
    readonly string? testMedia, smokeOut;
    readonly string lrclibBase;   // https://lrclib.net; a self-test can point it at a fake one (--lrclib)
    static readonly HttpClient http = new() { Timeout = TimeSpan.FromSeconds(10) };
    // Ollama on this PC, for translating lyrics (nothing leaves the PC); a self-test points it at a fake one (--ollama)
    readonly string ollamaBase;
    static readonly HttpClient ollamaHttp = new(new HttpClientHandler { AllowAutoRedirect = false, UseProxy = false }) { Timeout = System.Threading.Timeout.InfiniteTimeSpan };
    Process? ollamaServe;   // an Ollama String started itself: stopped again when String closes
    readonly Dictionary<string, CancellationTokenSource> chats = new();
    readonly bool smoke;
    readonly int smokeTimeout;
    // Navidrome's database (its play history, for phone listening in Stats); a self-test only ever reads its own fake
    // one (--navidrome-db), never a real one
    readonly string? navidromeDbArg;
    CoreWebView2Environment? env;
    WindowState lastState = WindowState.Normal;
    bool loaded, initStarted, closing, closeNow, hidden, profileRead, profileReady, closeAnyway, finishingClose, flushAnswered;
    DispatcherTimer? closeTimer;
    // saves of state.json and the other <name>.json run one after another off the window's thread; a name whose last
    // write failed stays here (with why) until a write of it works, and closing waits for them and checks
    Task storeChain = Task.CompletedTask;
    readonly System.Collections.Concurrent.ConcurrentDictionary<string, string> saveErrors = new();
    bool smokeBreakSaves;   // self-test only: every save fails, as on a full or locked disk
    readonly List<string> smokeLog = new();
    string LookDir => Path.Combine(dataDir, "look");
    FileSystemWatcher? lookWatch, lookDirWatch;
    DispatcherTimer? lookTimer;

    public MainWindow(string[] args)
    {
        smoke = args.Contains("--smoke-test");
        smokeOut = Arg(args, "--out");
        smokeTimeout = int.TryParse(Arg(args, "--timeout"), out var secs) ? secs : 240;
        testMedia = Arg(args, "--test-media") is string tm ? Path.GetFullPath(tm) : null;
        lrclibBase = (smoke ? Arg(args, "--lrclib") : null) ?? "https://lrclib.net";
        ollamaBase = (smoke ? Arg(args, "--ollama") : null) ?? "http://127.0.0.1:11434";
        navidromeDbArg = smoke ? Arg(args, "--navidrome-db") : null;
        updateFeedArg = smoke ? Arg(args, "--update-feed") : null;   // self-tests: a fake GitHub, and its own release key
        updateKeyArg = smoke ? Arg(args, "--update-key") : null;
        dataDir = Path.GetFullPath(Arg(args, "--data") ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "String"));
        webRoot = Path.Combine(AppContext.BaseDirectory, "wwwroot");
        placementPath = Path.Combine(dataDir, "window.json");
        backupDir = smoke ? Path.Combine(dataDir, "backup") : Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments), "String Backup");
        Directory.CreateDirectory(dataDir);

        Title = AppName;
        Icon = MakeIcon();
        Width = 1400; Height = 880; MinWidth = 960; MinHeight = 600;
        Background = new SolidColorBrush(Color.FromRgb(0x05, 0x05, 0x06));
        web.DefaultBackgroundColor = System.Drawing.Color.FromArgb(0x05, 0x05, 0x06);   // no white flash while it loads
        Content = web;

        if (smoke)
        {
            WindowStartupLocation = WindowStartupLocation.Manual;
            Left = -20000; Top = -20000; ShowActivated = false; ShowInTaskbar = false;
        }
        else RestorePlacement();

        SetupThumbButtons();
        StateChanged += (_, _) =>
        {
            if (WindowState != WindowState.Minimized) lastState = WindowState;
            bool nowHidden = WindowState == WindowState.Minimized;
            if (nowHidden != hidden) { hidden = nowHidden; Post(new { kind = hidden ? "hide" : "show" }); }
        };
        SourceInitialized += (_, _) => StyleTitleBar();
        Loaded += async (_, _) => { if (!initStarted) { initStarted = true; await InitAsync(); } };
        Closing += OnClosing;
        Closed += (_, _) => KillAll();
    }

    static string? Arg(string[] args, string name)
    {
        int i = Array.IndexOf(args, name);
        return i >= 0 && i + 1 < args.Length ? args[i + 1] : null;
    }

    async Task InitAsync()
    {
        try
        {
            env = await CoreWebView2Environment.CreateAsync(null, Path.Combine(dataDir, "webview"));
            await web.EnsureCoreWebView2Async(env);
        }
        catch (Exception ex)
        {
            MessageBox.Show(this, AppName + " needs the Microsoft Edge WebView2 runtime.\n\n" + ex.Message, AppName);
            return;
        }
        var core = web.CoreWebView2;
        var s = core.Settings;
        s.IsStatusBarEnabled = false;
        s.IsZoomControlEnabled = false;
        s.AreDefaultContextMenusEnabled = false;
        s.AreBrowserAcceleratorKeysEnabled = false;   // no Ctrl+R reload, no Ctrl+P print
        s.AreDevToolsEnabled = false;
        s.IsPasswordAutosaveEnabled = false;
        s.IsGeneralAutofillEnabled = false;
        core.AddWebResourceRequestedFilter(Origin + "*", CoreWebView2WebResourceContext.All);
        core.WebResourceRequested += Serve;
        core.WebMessageReceived += OnMessage;
        core.NewWindowRequested += (_, e) => { e.Handled = true; OpenExternal(e.Uri); };
        core.NavigationStarting += (_, e) => { if (!e.Uri.StartsWith(Origin, StringComparison.OrdinalIgnoreCase)) e.Cancel = true; };
        core.Navigate(Origin + "index.html");
        WatchLook();
    }

    // The look folder is watched while it exists: a saved change reaches the page (which loads look.css again) a moment
    // later. The data folder is watched for the look folder itself appearing, going or being replaced (restored from a
    // backup, say), and a watcher that fails is set up again.
    void WatchLook()
    {
        if (lookDirWatch == null && Directory.Exists(dataDir))
        {
            try
            {
                var d = new FileSystemWatcher(dataDir, "look") { IncludeSubdirectories = false, NotifyFilter = NotifyFilters.DirectoryName };
                d.Created += (_, _) => RebindLook();
                d.Deleted += (_, _) => RebindLook();
                d.Renamed += (_, _) => RebindLook();
                d.Error += (_, _) => RebindLook();
                d.EnableRaisingEvents = true;
                lookDirWatch = d;
            }
            catch { }
        }
        if (lookWatch != null || !Directory.Exists(LookDir)) return;
        try
        {
            var w = new FileSystemWatcher(LookDir) { IncludeSubdirectories = true, NotifyFilter = NotifyFilters.FileName | NotifyFilters.DirectoryName | NotifyFilters.LastWrite | NotifyFilters.Size };
            w.Changed += (_, _) => LookChanged();
            w.Created += (_, _) => LookChanged();
            w.Deleted += (_, _) => LookChanged();
            w.Renamed += (_, _) => LookChanged();
            w.Error += (_, _) => RebindLook();
            w.EnableRaisingEvents = true;
            lookWatch = w;
        }
        catch { }
    }

    void RebindLook() => Dispatcher.BeginInvoke(() =>
    {
        lookWatch?.Dispose();
        lookWatch = null;
        lookDirWatch?.Dispose();   // either may be the one that failed: both start over
        lookDirWatch = null;
        WatchLook();
        LookChanged();
    });

    void LookChanged() => Dispatcher.BeginInvoke(() =>
    {
        lookTimer ??= new DispatcherTimer(TimeSpan.FromMilliseconds(250), DispatcherPriority.Normal, (_, _) => { lookTimer!.Stop(); Post(new { kind = "look" }); }, Dispatcher);
        lookTimer.Stop();
        lookTimer.Start();
    });

    /* ---------- serving the page, and songs from anywhere ---------- */
    void Serve(object? sender, CoreWebView2WebResourceRequestedEventArgs e)
    {
        var uri = new Uri(e.Request.Uri);
        var rel = Uri.UnescapeDataString(uri.AbsolutePath).TrimStart('/');
        if (rel.StartsWith("_media/")) { ServeMedia(e, rel[7..]); return; }
        if (rel.StartsWith("_look/")) { ServeLook(e, rel[6..]); return; }
        if (rel.StartsWith("_addon/")) { ServeAddon(e, rel[7..]); return; }
        var full = Path.GetFullPath(Path.Combine(webRoot, rel.Length == 0 ? "index.html" : rel));
        if (!full.StartsWith(Path.GetFullPath(webRoot) + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase) || !File.Exists(full))
        {
            e.Response = env!.CreateWebResourceResponse(null, 404, "Not Found", "");
            return;
        }
        var stream = new FileStream(full, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
        e.Response = env!.CreateWebResourceResponse(stream, 200, "OK", $"Content-Type: {Mime(full)}\r\nCache-Control: no-store");
    }

    // /_media/<full path, '/'-separated, each part URL-encoded>: sounds and pictures only, read-only, Range answered with 206
    static readonly HashSet<string> MediaExt = new(StringComparer.OrdinalIgnoreCase)
    {
        ".mp3", ".flac", ".m4a", ".m4b", ".aac", ".ogg", ".oga", ".opus", ".wav", ".weba", ".webm", ".mp4",
        ".jpg", ".jpeg", ".png", ".webp", ".gif", ".bmp", ".avif",
    };
    static readonly Regex RangeRx = new(@"^\s*bytes\s*=\s*(\d*)\s*-\s*(\d*)\s*$", RegexOptions.Compiled);

    void ServeMedia(CoreWebView2WebResourceRequestedEventArgs e, string path)
    {
        string? full = null;
        try { path = path.Replace('/', '\\'); if (Path.IsPathFullyQualified(path)) full = Path.GetFullPath(path); } catch { }
        if (full is null || !MediaExt.Contains(Path.GetExtension(full)) || !File.Exists(full))
        {
            e.Response = env!.CreateWebResourceResponse(null, 404, "Not Found", "");
            return;
        }
        // the Range header is read before the file is opened: a number too big for a long is answered 416, never an
        // exception with the file still open
        var range = e.Request.Headers.Contains("Range") ? e.Request.Headers.GetHeader("Range") : null;
        var m = range is null ? null : RangeRx.Match(range);
        bool ranged = m is not null && m.Success && (m.Groups[1].Length > 0 || m.Groups[2].Length > 0);
        long first = -1, last = -1;
        bool rangeOk = !ranged
            || ((m!.Groups[1].Length == 0 || long.TryParse(m.Groups[1].Value, out first))
                && (m.Groups[2].Length == 0 || long.TryParse(m.Groups[2].Value, out last)));
        FileStream fs;
        try { fs = new FileStream(full, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete); }
        catch { e.Response = env!.CreateWebResourceResponse(null, 404, "Not Found", ""); return; }
        long len = fs.Length;
        var head = $"Content-Type: {Mime(full)}\r\nAccept-Ranges: bytes\r\nCache-Control: no-store";
        if (!ranged)
        {
            e.Response = env!.CreateWebResourceResponse(fs, 200, "OK", $"{head}\r\nContent-Length: {len}");
            return;
        }
        long start = 0, end = -1;
        if (rangeOk)
        {
            if (first < 0) { start = Math.Max(0, len - last); end = len - 1; }   // "bytes=-N": the last N bytes
            else { start = first; end = last >= 0 ? Math.Min(last, len - 1) : len - 1; }
        }
        if (!rangeOk || start >= len || end < start)
        {
            fs.Dispose();
            e.Response = env!.CreateWebResourceResponse(null, 416, "Range Not Satisfiable", $"Content-Range: bytes */{len}");
            return;
        }
        // always wrapped: WebView2 rewinds a seekable stream before reading it, which would send the start of the file
        fs.Seek(start, SeekOrigin.Begin);
        var body = new SliceStream(fs, end - start + 1);
        e.Response = env!.CreateWebResourceResponse(body, 206, "Partial Content", $"{head}\r\nContent-Length: {end - start + 1}\r\nContent-Range: bytes {start}-{end}/{len}");
    }

    // /_look/[v<n>/]<file>: the person's own look from <data>\look\ (a stylesheet, pictures, fonts), read-only and nothing
    // outside that folder, never through a link inside it. The page puts a fresh v<n> in each reload's address, so the
    // pictures a stylesheet names load fresh too. A missing look.css is an empty one, so a String without a look shows
    // no error.
    static readonly Regex LookRev = new(@"^v\d+/", RegexOptions.Compiled);
    static readonly HashSet<string> LookExt = new(StringComparer.OrdinalIgnoreCase)
    {
        ".css", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif", ".bmp", ".svg", ".ico", ".woff", ".woff2", ".ttf", ".otf",
    };

    void ServeLook(CoreWebView2WebResourceRequestedEventArgs e, string rel)
    {
        string? full = null;
        rel = LookRev.Replace(rel, "", 1);
        try { if (!Path.IsPathRooted(rel)) full = Path.GetFullPath(Path.Combine(LookDir, rel.Replace('/', '\\'))); } catch { }
        if (full is null || !Inside(full, LookDir) || !LookExt.Contains(Path.GetExtension(full)) || LinkInside(LookDir, full))
        {
            e.Response = env!.CreateWebResourceResponse(null, 404, "Not Found", "");
            return;
        }
        if (!File.Exists(full))
        {
            e.Response = string.Equals(full, Path.Combine(LookDir, "look.css"), StringComparison.OrdinalIgnoreCase)
                ? env!.CreateWebResourceResponse(new MemoryStream(), 200, "OK", "Content-Type: text/css; charset=utf-8\r\nCache-Control: no-store")
                : env!.CreateWebResourceResponse(null, 404, "Not Found", "");
            return;
        }
        // read whole and closed at once: an editor saving the file a moment later must never find it held open
        try
        {
            var ms = new MemoryStream();
            using (var fs = new FileStream(full, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete))
            {
                if (fs.Length > 64L << 20) throw new IOException("Too big for a look file");
                fs.CopyTo(ms);
            }
            ms.Position = 0;
            e.Response = env!.CreateWebResourceResponse(ms, 200, "OK", $"Content-Type: {Mime(full)}\r\nCache-Control: no-store");
        }
        catch { e.Response = env!.CreateWebResourceResponse(null, 404, "Not Found", ""); }
    }

    // /_addon/<id>/<file>: an add-on's scripts, styles and pictures from <data>\addons\<id>\ (see string.js), read-only,
    // nothing outside its folder and never through a link inside it; read whole, like the look's files
    string AddonsDir => Path.Combine(dataDir, "addons");
    static readonly HashSet<string> AddonExt = new(StringComparer.OrdinalIgnoreCase)
    {
        ".js", ".css", ".json", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".woff", ".woff2", ".ttf", ".otf",
    };
    static readonly Regex AddonId = new(@"^[a-z0-9][a-z0-9-]{0,40}$", RegexOptions.Compiled);
    void ServeAddon(CoreWebView2WebResourceRequestedEventArgs e, string rel)
    {
        string? full = null, dir = null;
        var slash = rel.IndexOf('/');
        if (slash > 0 && AddonId.IsMatch(rel[..slash]))
        {
            dir = Path.Combine(AddonsDir, rel[..slash]);
            try { var f = rel[(slash + 1)..]; if (f.Length > 0 && !Path.IsPathRooted(f)) full = Path.GetFullPath(Path.Combine(dir, f.Replace('/', '\\'))); } catch { }
        }
        if (full is null || dir is null || !Inside(full, dir) || !AddonExt.Contains(Path.GetExtension(full)) || LinkInside(dataDir, full) || !File.Exists(full))
        {
            e.Response = env!.CreateWebResourceResponse(null, 404, "Not Found", "");
            return;
        }
        try
        {
            var ms = new MemoryStream();
            using (var fs = new FileStream(full, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete))
            {
                if (fs.Length > 16L << 20) throw new IOException("Too big for an add-on file");
                fs.CopyTo(ms);
            }
            ms.Position = 0;
            e.Response = env!.CreateWebResourceResponse(ms, 200, "OK", $"Content-Type: {Mime(full)}\r\nCache-Control: no-store");
        }
        catch { e.Response = env!.CreateWebResourceResponse(null, 404, "Not Found", ""); }
    }

    // the add-ons installed: each folder in <data>\addons\ with an addon.json ({ name, version, scripts, styles }); only
    // file names inside the add-on's own folder are passed on, in the order given
    List<object> Addons()
    {
        var list = new List<object>();
        try
        {
            if (!Directory.Exists(AddonsDir) || LinkInside(dataDir, AddonsDir)) return list;
            foreach (var d in Directory.GetDirectories(AddonsDir).OrderBy(x => x, StringComparer.OrdinalIgnoreCase))
            {
                var id = Path.GetFileName(d);
                var manifest = Path.Combine(d, "addon.json");
                if (!AddonId.IsMatch(id) || LinkInside(dataDir, d) || !File.Exists(manifest) || LinkInside(dataDir, manifest)) continue;
                try
                {
                    var j = JsonNode.Parse(File.ReadAllText(manifest)) as JsonObject;
                    if (j is null) continue;
                    List<string> Files(string key, string ext) => (j[key] as JsonArray ?? new JsonArray())
                        .Select(x => (string?)x ?? "").Where(f => f.Length > 0 && !f.Contains("..") && !Path.IsPathRooted(f) && f.EndsWith(ext, StringComparison.OrdinalIgnoreCase)
                            && File.Exists(Path.Combine(d, f.Replace('/', '\\')))).ToList();
                    list.Add(new { id, name = (string?)j["name"] ?? id, version = (string?)j["version"] ?? "", scripts = Files("scripts", ".js"), styles = Files("styles", ".css") });
                }
                catch { }   // a broken add-on is skipped, String starts as usual
            }
        }
        catch { }
        return list;
    }

    // true if the file, or a folder between it and root, is a link (symlink or junction)
    static bool LinkInside(string root, string full)
    {
        try
        {
            root = Path.GetFullPath(root).TrimEnd('\\');
            for (var p = full; p.Length > root.Length && p.StartsWith(root, StringComparison.OrdinalIgnoreCase); p = Path.GetDirectoryName(p) ?? root)
                if ((File.Exists(p) || Directory.Exists(p)) && (File.GetAttributes(p) & FileAttributes.ReparsePoint) != 0) return true;
            return false;
        }
        catch { return true; }
    }

    sealed class SliceStream(Stream inner, long length) : Stream
    {
        readonly long total = length;
        long left = length;
        public override bool CanRead => true;
        public override bool CanSeek => false;
        public override bool CanWrite => false;
        public override long Length => total;
        public override long Position { get => total - left; set => throw new NotSupportedException(); }
        public override int Read(byte[] buffer, int offset, int count)
        {
            if (left <= 0) return 0;
            int n = inner.Read(buffer, offset, (int)Math.Min(count, left));
            left -= n;
            return n;
        }
        public override void Flush() { }
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
        protected override void Dispose(bool disposing) { if (disposing) inner.Dispose(); base.Dispose(disposing); }
    }

    static string Mime(string path) => Path.GetExtension(path).ToLowerInvariant() switch
    {
        ".html" or ".htm" => "text/html; charset=utf-8",
        ".css" => "text/css; charset=utf-8",
        ".js" or ".mjs" => "text/javascript; charset=utf-8",
        ".json" => "application/json; charset=utf-8",
        ".svg" => "image/svg+xml",
        ".png" => "image/png",
        ".jpg" or ".jpeg" => "image/jpeg",
        ".gif" => "image/gif",
        ".webp" => "image/webp",
        ".avif" => "image/avif",
        ".bmp" => "image/bmp",
        ".ico" => "image/x-icon",
        ".woff" => "font/woff",
        ".woff2" => "font/woff2",
        ".ttf" => "font/ttf",
        ".otf" => "font/otf",
        ".mp3" => "audio/mpeg",
        ".wav" => "audio/wav",
        ".ogg" or ".oga" or ".opus" => "audio/ogg",
        ".flac" => "audio/flac",
        ".m4a" or ".m4b" => "audio/mp4",
        ".aac" => "audio/aac",
        ".weba" => "audio/webm",
        ".mp4" => "video/mp4",
        ".webm" => "video/webm",
        _ => "application/octet-stream",
    };

    /* ---------- messages from the page (wwwroot\host.js) ---------- */
    void OnMessage(object? sender, CoreWebView2WebMessageReceivedEventArgs e)
    {
        if (!e.Source.StartsWith(Origin, StringComparison.OrdinalIgnoreCase)) return;
        JsonDocument doc;
        try { doc = JsonDocument.Parse(e.WebMessageAsJson); } catch { return; }
        using (doc)
        {
            var m = doc.RootElement;
            string S(string k) => m.TryGetProperty(k, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString()! : "";
            switch (S("kind"))
            {
                case "hello":
                    Post(new { kind = "hello", smoke });
                    if (!loaded) { loaded = true; if (smoke) _ = RunSmokeAsync(); }
                    break;
                case "load":
                {
                    profileRead = profileReady = false;
                    try
                    {
                        var state = ReadProfileDocument("state");
                        var listening = ReadProfileDocument("listening");
                        profileRead = true;
                        Post(new { kind = "reply", req = S("req"), data = new { state, listening } });
                    }
                    catch (Exception ex) { Post(new { kind = "reply", req = S("req"), data = new { error = ex.Message } }); }
                    break;
                }
                case "profileReady": profileReady = profileRead; break;
                case "save":
                case "store":
                {
                    // every save is answered ("saved", ok or not), so the page can keep what didn't reach the disk
                    var name = S("kind") == "save" ? "state" : S("name");
                    long seq = m.TryGetProperty("seq", out var sq) && sq.ValueKind == JsonValueKind.Number && sq.TryGetInt64(out var sn) ? sn : 0;
                    if (!profileReady) { Post(new { kind = "saved", name, seq, ok = false, refused = true, error = "your library isn't open yet" }); break; }
                    if (!Regex.IsMatch(name, "^[a-z][a-z0-9-]{0,30}$") || !m.TryGetProperty("data", out var sd)) { Post(new { kind = "saved", name, seq, ok = false, refused = true, error = "not something String saves" }); break; }
                    QueueStore(name, sd.GetRawText(), seq);
                    break;
                }
                case "closeAnyway": closeAnyway = true; Close(); break;
                case "title": Title = S("text").Length > 0 ? $"{S("text")} · {AppName}" : AppName; break;
                case "playing": SetThumbPlaying(m.TryGetProperty("on", out var on) && on.ValueKind == JsonValueKind.True); break;
                case "open": OpenExternal(S("url")); break;
                case "flushed": if (closing) { flushAnswered = true; _ = FinishCloseAsync(); } break;
                case "native":
                    if (S("op") == "dropped")   // files dropped from Explorer: their paths ride along as File objects
                    {
                        Post(new { kind = "reply", req = S("req"), data = new { ok = true, value = DroppedPaths(e), error = (string?)null } });
                        break;
                    }
                    var args = m.TryGetProperty("args", out var a) ? a.Clone() : default;   // Clone: the message is disposed before the work finishes
                    _ = NativeAsync(S("req"), S("op"), args);
                    break;
            }
        }
    }

    static List<string> DroppedPaths(CoreWebView2WebMessageReceivedEventArgs e)
    {
        var list = new List<string>();
        try
        {
            if (e.AdditionalObjects is not { } objs) return list;
            foreach (var o in objs)
            {
                var p = o switch { CoreWebView2File f => f.Path, CoreWebView2FileSystemHandle h => h.Path, _ => null };
                if (!string.IsNullOrEmpty(p) && Path.IsPathFullyQualified(p)) list.Add(Path.GetFullPath(p));
            }
        }
        catch { }
        return list;
    }

    void Post(object msg) { try { web.CoreWebView2?.PostWebMessageAsJson(JsonSerializer.Serialize(msg)); } catch { } }

    static void Try(Action a) { try { a(); } catch { } }

    JsonObject? ReadProfileDocument(string name)
    {
        var path = Path.Combine(dataDir, name + ".json");
        try
        {
            var doc = JsonNode.Parse(File.ReadAllText(path)) as JsonObject;
            if (doc == null || (int?)doc["v"] != 1 || (name == "listening" && doc["days"] is not JsonObject))
                throw new InvalidDataException($"Unsupported data in {name}.json");
            return doc;
        }
        catch (FileNotFoundException) { return null; }
    }

    // A whole file at once: written beside it under a name of its own, then swapped in. Writes of the same file wait for
    // each other (two at once would otherwise trip over the swap), each with its own temporary name.
    static readonly System.Collections.Concurrent.ConcurrentDictionary<string, object> writeLocks = new(StringComparer.OrdinalIgnoreCase);
    static long tmpSeq;
    static void WriteAtomic(string path, string text)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        lock (writeLocks.GetOrAdd(Path.GetFullPath(path), _ => new object()))
        {
            var tmp = $"{path}.{Environment.ProcessId}-{Interlocked.Increment(ref tmpSeq)}.tmp";
            try
            {
                File.WriteAllText(tmp, text);
                if (File.Exists(path)) File.Replace(tmp, path, path + ".bak");
                else File.Move(tmp, path);
            }
            finally { try { if (File.Exists(tmp)) File.Delete(tmp); } catch { } }
        }
    }

    void QueueStore(string name, string json, long seq)
    {
        storeChain = storeChain.ContinueWith(_ =>
        {
            var error = Store(name, json);
            if (error is null) saveErrors.TryRemove(name, out string? _); else saveErrors[name] = error;
            Dispatcher.BeginInvoke(() => Post(new { kind = "saved", name, seq, ok = error is null, error }));
        }, TaskScheduler.Default);
    }

    // <name>.json in the data folder, plus one dated copy per day in Documents\String Backup (the newest 60 of each kept).
    // Runs off the window's thread (a slow or synced Documents folder can't freeze the player). Answers why it failed.
    string? Store(string name, string json)
    {
        try
        {
            if (smokeBreakSaves) throw new IOException("The disk is full (a self-test's pretend failure)");
            WriteAtomic(Path.Combine(dataDir, name + ".json"), json);
        }
        catch (Exception ex) { LogError("save " + name, ex); return ex.Message; }
        try
        {
            Directory.CreateDirectory(backupDir);
            File.WriteAllText(Path.Combine(backupDir, $"{name}-{DateTime.Now:yyyy-MM-dd}.json"), json);
            foreach (var old in Directory.GetFiles(backupDir, $"{name}-*.json").OrderByDescending(f => f).Skip(60))
                File.Delete(old);
        }
        catch { }
        return null;
    }

    // <data>\errors.log: unexpected errors and failed saves, newest last (over 512 KB, it starts again, the old one kept
    // as errors-previous.log)
    readonly object logLock = new();
    public void LogError(string where, Exception? ex)
    {
        try
        {
            lock (logLock)
            {
                var path = Path.Combine(dataDir, "errors.log");
                if (File.Exists(path) && new FileInfo(path).Length > 512 * 1024) File.Move(path, Path.Combine(dataDir, "errors-previous.log"), true);
                File.AppendAllText(path, $"{DateTime.Now:yyyy-MM-dd HH:mm:ss} {where}: {ex}\r\n\r\n");
            }
        }
        catch { }
    }

    void OpenExternal(string url)
    {
        if (smoke || !Uri.TryCreate(url, UriKind.Absolute, out var u) || (u.Scheme != "https" && u.Scheme != "http")) return;
        if (u.Host == Domain) return;
        try { Process.Start(new ProcessStartInfo(u.AbsoluteUri) { UseShellExecute = true }); } catch { }
    }

    /* ---------- files and programs for the page (belt.native in host.js) ----------
       Programs: only .exe files inside String's own folder or its data folder (where an add-on keeps its tools).
       Files: read and look anywhere; write and copy only into the data folder. Opening a file never runs a program. */
    readonly Dictionary<string, Process> procs = new();
    int procSeq;
    static readonly HashSet<string> RunnableExt = new(StringComparer.OrdinalIgnoreCase)
    {
        ".exe", ".com", ".bat", ".cmd", ".ps1", ".psm1", ".vbs", ".vbe", ".js", ".jse", ".wsf", ".wsh", ".msi", ".msp", ".scr",
        ".pif", ".lnk", ".url", ".hta", ".cpl", ".jar", ".reg", ".appref-ms", ".application", ".msc", ".gadget", ".inf",
    };
    static bool Inside(string path, string root) =>
        Path.GetFullPath(path).StartsWith(Path.GetFullPath(root).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase);
    static long Ms(DateTime utc) => (long)(utc - DateTime.UnixEpoch).TotalMilliseconds;

    async Task NativeAsync(string req, string op, JsonElement a)
    {
        object? value = null;
        string? error = null;
        try { value = await NativeOp(op, a); }
        catch (Exception ex) { error = ex.Message; }
        Post(new { kind = "reply", req, data = new { ok = error is null, value, error } });
    }

    async Task<object?> NativeOp(string op, JsonElement a)
    {
        string S(string k) => a.ValueKind == JsonValueKind.Object && a.TryGetProperty(k, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString()! : "";
        bool Flag(string k) => a.ValueKind == JsonValueKind.Object && a.TryGetProperty(k, out var v) && v.ValueKind == JsonValueKind.True;
        double N(string k) => a.ValueKind == JsonValueKind.Object && a.TryGetProperty(k, out var v) && v.ValueKind == JsonValueKind.Number ? v.GetDouble() : 0;
        string P(string k)
        {
            var s = S(k);
            if (s.Length == 0 || !Path.IsPathFullyQualified(s)) throw new ArgumentException($"{k} must be a full path");
            return Path.GetFullPath(s);
        }
        string Folder(Environment.SpecialFolder f) => Environment.GetFolderPath(f);
        switch (op)
        {
            case "addons": return await Task.Run(Addons);
            case "updateInfo": return UpdateInfo();
            case "updateCheck": return await UpdateCheck();
            case "updateInstall": return await UpdateInstall(S("version"));
            case "smokeUpdate":
            {
                // self-tests only: another fake release feed (a tampered one, an older one)
                if (!smoke) throw new NotSupportedException("Only in a smoke test");
                updateFeedArg = S("feed");
                return true;
            }
            case "paths":
                return new
                {
                    tool = AppContext.BaseDirectory.TrimEnd('\\'), data = dataDir, look = LookDir, addons = AddonsDir,
                    home = Folder(Environment.SpecialFolder.UserProfile),
                    music = Folder(Environment.SpecialFolder.MyMusic),
                    documents = Folder(Environment.SpecialFolder.MyDocuments),
                    downloads = Path.Combine(Folder(Environment.SpecialFolder.UserProfile), "Downloads"),
                    testMedia,
                };
            case "features": return new { dropped = true, multiPick = true, media = true };
            case "run": return StartProc(a);
            case "kill":
                if (procs.TryGetValue(S("proc"), out var pr)) Try(() => pr.Kill(true));
                if (chats.TryGetValue(S("proc"), out var cc)) Try(() => cc.Cancel());
                return true;
            case "ollamaModels":
            {
                // the models Ollama has downloaded (starting Ollama if it isn't running)
                await EnsureOllama();
                var j = JsonNode.Parse(await http.GetStringAsync(ollamaBase + "/api/tags"));
                // each model checked at the same time (one after another took longer with every model installed)
                var names = (j?["models"]?.AsArray() ?? new JsonArray()).Select(tag => (string?)tag?["name"]).OfType<string>().ToList();
                var local = await Task.WhenAll(names.Select(IsLocalModel));
                return names.Where((_, i) => local[i]).ToList();
            }
            case "ollamaChat":
            {
                // a chat with a local model, streamed: each line of Ollama's reply (NDJSON) goes to the page as it comes,
                // like a program's output (belt.native.ollamaChat in host.js); "kill" stops it
                var body = S("body");
                if (body.Length == 0 || body.Length > 400_000) throw new ArgumentException("body must be a chat request");
                await EnsureOllama();
                var model = (string?)JsonNode.Parse(body)?["model"];
                if (model == null || !await IsLocalModel(model))
                    throw new InvalidOperationException("Choose an installed local model in Playback to translate privately.");
                var handle = "o" + ++procSeq;
                var cts = new CancellationTokenSource();
                chats[handle] = cts;
                _ = Task.Run(async () =>
                {
                    int code = 0;
                    void Line(string text) => Dispatcher.BeginInvoke(new Action(() => Post(new { kind = "proc", proc = handle, ev = "line", stream = "out", text })));
                    try
                    {
                        using var req = new HttpRequestMessage(HttpMethod.Post, ollamaBase + "/api/chat") { Content = new StringContent(body, Encoding.UTF8, "application/json") };
                        using var res = await ollamaHttp.SendAsync(req, HttpCompletionOption.ResponseHeadersRead, cts.Token);
                        if (!res.IsSuccessStatusCode)
                        {
                            Line(JsonSerializer.Serialize(new { error = $"Ollama answered {(int)res.StatusCode}: {await res.Content.ReadAsStringAsync(cts.Token)}" }));
                            code = 1;
                        }
                        else
                        {
                            using var rd = new StreamReader(await res.Content.ReadAsStreamAsync(cts.Token), Encoding.UTF8);
                            string? l;
                            while ((l = await rd.ReadLineAsync(cts.Token)) is not null) if (l.Length > 0) Line(l);
                        }
                    }
                    catch (OperationCanceledException) { code = 2; }
                    catch (Exception ex) { Line(JsonSerializer.Serialize(new { error = ex.Message })); code = 1; }
                    _ = Dispatcher.BeginInvoke(new Action(() => { chats.Remove(handle); cts.Dispose(); Post(new { kind = "proc", proc = handle, ev = "exit", code }); }));
                });
                return handle;
            }
            case "stat": return Stat(P("path"));
            case "stats":
                if (!a.TryGetProperty("paths", out var list) || list.ValueKind != JsonValueKind.Array) throw new ArgumentException("paths must be a list");
                var many = list.EnumerateArray().Select(x => x.GetString() ?? "").ToList();
                return await Task.Run(() => many.Select(x => Path.IsPathFullyQualified(x) ? Stat(Path.GetFullPath(x)) : Stat("")).ToList());
            case "list":
            {
                var dir = P("path");
                return await Task.Run(() => Directory.Exists(dir)
                    ? new DirectoryInfo(dir).EnumerateFileSystemInfos().Take(20000).Select(f => new { name = f.Name, dir = f is DirectoryInfo, size = f is FileInfo fi ? fi.Length : 0, mtime = Ms(f.LastWriteTimeUtc) }).ToList<object>()
                    : new List<object>());
            }
            case "readText":
            {
                var p = P("path");
                var fi = new FileInfo(p);
                if (!fi.Exists) return null;
                if (fi.Length > 32 << 20) throw new IOException("That file is too large to read as text");
                return await File.ReadAllTextAsync(p);
            }
            case "writeText":
            {
                var p = P("path");
                if (!Inside(p, dataDir)) throw new UnauthorizedAccessException("Writing is only allowed inside String's data folder");
                var text = S("text");
                await Task.Run(() => WriteAtomic(p, text));
                return true;
            }
            case "copy":
            {
                // a file into the data folder (self-test songs, an add-on's files)
                string src = P("src"), dst = P("dst");
                if (!Inside(dst, dataDir)) throw new UnauthorizedAccessException("Copying is only allowed into String's data folder");
                await Task.Run(() => { Directory.CreateDirectory(Path.GetDirectoryName(dst)!); File.Copy(src, dst, true); });
                return dst;
            }
            case "replaceSong":
            {
                // a song file swapped for new bytes (an add-on writing names into a file it made, having checked the new
                // bytes first). The original is copied to <data>\tag-backup\ before anything changes (the first original
                // stays there), then the new file takes its place in one step. Song files only, and only existing ones.
                var p = P("path");
                if (!Mime(p).StartsWith("audio/")) throw new UnauthorizedAccessException("Only song files can be rewritten");
                if (!File.Exists(p)) throw new FileNotFoundException("The file isn't there any more");
                var bytes = Convert.FromBase64String(S("data"));
                if (bytes.Length < 4096) throw new ArgumentException("That isn't a whole song");
                var backupDir = Path.Combine(dataDir, "tag-backup");
                var tag = Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(Encoding.UTF8.GetBytes(p.ToLowerInvariant())))[..8];
                var backup = Path.Combine(backupDir, $"{tag} {Path.GetFileName(p)}");
                await Task.Run(() =>
                {
                    Directory.CreateDirectory(backupDir);
                    if (!File.Exists(backup)) File.Copy(p, backup);
                    var tmp = p + ".string-new";
                    File.WriteAllBytes(tmp, bytes);
                    try { File.Replace(tmp, p, null); }
                    catch { try { File.Delete(tmp); } catch { } throw; }
                });
                return new { path = p, backup };
            }
            case "recycle":
            {
                // the Duplicates page's "Move to the Recycle Bin", after you say yes: song files only, each to the
                // Recycle Bin (never deleted outright: Windows asks first if a drive has no Recycle Bin). The self-test
                // never touches the real Recycle Bin: there the files move into the test's own data folder.
                if (!a.TryGetProperty("paths", out var toRecycle) || toRecycle.ValueKind != JsonValueKind.Array) throw new ArgumentException("paths must be a list");
                var done = new List<string>();
                var failed = new List<object>();
                foreach (var x in toRecycle.EnumerateArray())
                {
                    var p = x.GetString() ?? "";
                    try
                    {
                        if (!Path.IsPathFullyQualified(p)) throw new ArgumentException("Not a full path");
                        p = Path.GetFullPath(p);
                        if (!Mime(p).StartsWith("audio/")) throw new UnauthorizedAccessException("Only song files can be moved to the Recycle Bin");
                        if (!File.Exists(p)) throw new FileNotFoundException("The file isn't there any more");
                        if (smoke)
                        {
                            var to = Path.Combine(dataDir, "recycled-selftest");
                            Directory.CreateDirectory(to);
                            await Task.Run(() => File.Move(p, Path.Combine(to, Path.GetFileName(p)), true));
                        }
                        else await Task.Run(() => Recycle(p));
                        done.Add(p);
                    }
                    catch (Exception e) { failed.Add(new { path = p, error = e.Message }); }
                }
                return new { done, failed };
            }
            case "mkdir":
            {
                // a folder for downloaded songs (only ever creates; never removes or renames anything)
                var p = P("path");
                Directory.CreateDirectory(p);
                return true;
            }
            case "pickFolder":
            {
                if (smoke) return null;
                var dlg = new OpenFolderDialog { Title = S("title").Length > 0 ? S("title") : "Choose a folder", Multiselect = Flag("multi") };
                if (Directory.Exists(S("start"))) dlg.InitialDirectory = S("start");
                return dlg.ShowDialog(this) == true ? (dlg.Multiselect ? dlg.FolderNames : dlg.FolderName) : null;
            }
            case "pickFile":
            {
                if (smoke) return null;
                var dlg = new OpenFileDialog { Title = S("title").Length > 0 ? S("title") : "Choose a file", Filter = S("filter").Length > 0 ? S("filter") : "All files|*.*", Multiselect = Flag("multi") };
                if (Directory.Exists(S("start"))) dlg.InitialDirectory = S("start");
                return dlg.ShowDialog(this) == true ? (dlg.Multiselect ? dlg.FileNames : dlg.FileName) : null;
            }
            case "open":
            {
                var p = P("path");
                if (!File.Exists(p) && !Directory.Exists(p)) throw new FileNotFoundException("It isn't there any more");
                if (File.Exists(p) && RunnableExt.Contains(Path.GetExtension(p))) throw new UnauthorizedAccessException("String doesn't open programs");
                if (!smoke) Process.Start(new ProcessStartInfo(p) { UseShellExecute = true });
                return true;
            }
            case "lookFolder":
            {
                // the person's own look: <data>\look\, made the first time with a starter look.css (examples, all switched
                // off), then shown in Explorer with look.css selected (not in a self-test, or with quiet)
                var css = Path.Combine(LookDir, "look.css");
                var made = false;
                if (!File.Exists(css))
                {
                    Directory.CreateDirectory(LookDir);
                    File.Copy(Path.Combine(webRoot, "look-starter.css"), css);
                    made = true;
                }
                lookWatch?.Dispose();   // the folder may be new, or a different one than the watcher had
                lookWatch = null;
                WatchLook();
                if (!smoke && !Flag("quiet")) Process.Start(new ProcessStartInfo("explorer.exe", $"/select,\"{css}\"") { UseShellExecute = true });
                return new { path = LookDir, made };
            }
            case "reveal":
            {
                var p = P("path");
                if (!smoke)
                {
                    if (File.Exists(p)) Process.Start(new ProcessStartInfo("explorer.exe", $"/select,\"{p}\"") { UseShellExecute = true });
                    else if (Directory.Exists(p)) Process.Start(new ProcessStartInfo("explorer.exe", $"\"{p}\"") { UseShellExecute = true });
                    else throw new FileNotFoundException("It isn't there any more");
                }
                return true;
            }
            case "smokeClick":
            {
                // self-tests only: a real mouse click (DevTools protocol), so the page gets the activation a person's click gives
                if (!smoke) throw new NotSupportedException("Only in a smoke test");
                double x = N("x"), y = N("y");
                foreach (var type in new[] { "mousePressed", "mouseReleased" })
                    await web.CoreWebView2.CallDevToolsProtocolMethodAsync("Input.dispatchMouseEvent", JsonSerializer.Serialize(new { type, x, y, button = "left", clickCount = 1 }));
                return true;
            }
            case "navidromePlays":
            {
                // the songs played through Navidrome (your phone), from its own history table, read-only:
                // { found, plays: [{ n, at (unix s), path, title, artist, duration }] } after play number `after`
                long after = a.TryGetProperty("after", out var av) && av.ValueKind == JsonValueKind.Number ? av.GetInt64() : 0;
                var requestedUser = S("user");
                var db = NavidromeDb();
                if (db == null || !File.Exists(db)) return new { found = false };
                return await Task.Run(() =>
                {
                    var plays = new List<object>();
                    var cs = new SqliteConnectionStringBuilder { DataSource = db, Mode = SqliteOpenMode.ReadOnly, Pooling = false, DefaultTimeout = 5 };
                    using var con = new SqliteConnection(cs.ToString());
                    con.Open();
                    var users = new Dictionary<string, string>();
                    using (var accounts = con.CreateCommand())
                    {
                        accounts.CommandText = "SELECT id, user_name FROM user ORDER BY user_name";
                        using var entries = accounts.ExecuteReader();
                        while (entries.Read()) users[entries.GetString(0)] = entries.GetString(1);
                    }
                    var user = requestedUser.Length == 0 && users.Count == 1 ? users.Keys.First() : requestedUser;
                    if (!users.ContainsKey(user)) return (object)new { found = true, users, user = "", plays };
                    using var cmd = con.CreateCommand();
                    cmd.CommandText = "SELECT s.id, s.submission_time, m.path, COALESCE(l.path, ''), COALESCE(m.title, ''), COALESCE(m.artist, ''), COALESCE(m.duration, 0) " +
                        "FROM scrobbles s JOIN media_file m ON m.id = s.media_file_id LEFT JOIN library l ON l.id = m.library_id " +
                        "WHERE s.user_id = $user AND s.id > $after ORDER BY s.id LIMIT 5000";
                    cmd.Parameters.AddWithValue("$after", after);
                    cmd.Parameters.AddWithValue("$user", user);
                    using var r = cmd.ExecuteReader();
                    while (r.Read())
                    {
                        string rel = r.IsDBNull(2) ? "" : r.GetString(2).Replace('/', '\\'), lib = r.GetString(3);
                        string full = Path.IsPathFullyQualified(rel) || lib.Length == 0 ? rel : Path.Combine(lib, rel);
                        plays.Add(new { n = r.GetInt64(0), at = r.GetInt64(1), path = full, title = r.GetString(4), artist = r.GetString(5), duration = r.GetDouble(6) });
                    }
                    return (object)new { found = true, users, user, plays };
                });
            }
            case "lrclib":
            {
                // lyrics from LRCLIB (lrclib.net), only once you said yes in String; only its /api/ addresses.
                // Asked from here so String can say who it is, as LRCLIB asks apps to
                var path = S("path");
                if (!path.StartsWith("/api/") || path.Contains("..") || path.Contains("://")) throw new ArgumentException("Not an LRCLIB address");
                using var req = new HttpRequestMessage(HttpMethod.Get, lrclibBase.TrimEnd('/') + path);
                req.Headers.TryAddWithoutValidation("User-Agent", "String v1.0 (personal music player)");
                req.Headers.TryAddWithoutValidation("Lrclib-Client", "String v1.0");
                using var res = await http.SendAsync(req);
                return new { status = (int)res.StatusCode, body = await res.Content.ReadAsStringAsync() };
            }
            case "smokeThumb":
            {
                // self-tests only: press a taskbar button (0 previous, 1 play/pause, 2 next; -1 just reads them)
                if (!smoke) throw new NotSupportedException("Only in a smoke test");
                int i = (int)N("i");
                if (i >= 0 && i < thumbActions.Length) thumbActions[i]();
                return new { buttons = TaskbarItemInfo?.ThumbButtonInfos.Count ?? 0, play = thumbPlay?.Description, playing = thumbPlaying };
            }
            case "smokeSaves":
            {
                // self-tests only: saves fail from now on (broken: true), or work again; answers how many names are unsaved
                if (!smoke) throw new NotSupportedException("Only in a smoke test");
                smokeBreakSaves = Flag("broken");
                return new { unsaved = saveErrors.Count };
            }
            case "smokeClose":
            {
                // self-tests only: close the window as a person would (the page saves first, then String closes or says why not)
                if (!smoke) throw new NotSupportedException("Only in a smoke test");
                Close();
                return true;
            }
            case "smokeDrop":
            {
                if (!smoke) throw new NotSupportedException("Only in a smoke test");
                if (!a.TryGetProperty("paths", out var hp) || hp.ValueKind != JsonValueKind.Array) throw new ArgumentException("paths must be a list");
                var files = hp.EnumerateArray().Select(x => Path.GetFullPath(x.GetString() ?? "")).Where(p => File.Exists(p) || Directory.Exists(p)).ToList();
                var drag = new { items = Array.Empty<object>(), files, dragOperationsMask = 1 };
                foreach (var type in new[] { "dragEnter", "dragOver", "drop" })
                    await web.CoreWebView2.CallDevToolsProtocolMethodAsync("Input.dispatchDragEvent", JsonSerializer.Serialize(new { type, x = N("x"), y = N("y"), data = drag }));
                return files.Count;
            }
            default: throw new NotSupportedException("Unknown operation: " + op);
        }
    }

    static object Stat(string p)
    {
        if (p.Length > 0 && File.Exists(p)) { var f = new FileInfo(p); return new { exists = true, dir = false, size = f.Length, mtime = Ms(f.LastWriteTimeUtc) }; }
        if (p.Length > 0 && Directory.Exists(p)) return new { exists = true, dir = true, size = 0L, mtime = Ms(Directory.GetLastWriteTimeUtc(p)) };
        return new { exists = false, dir = false, size = 0L, mtime = 0L };
    }

    string StartProc(JsonElement a)
    {
        string S(string k) => a.TryGetProperty(k, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString()! : "";
        var exe = S("exe");
        var full = Path.GetFullPath(Path.IsPathFullyQualified(exe) ? exe : Path.Combine(AppContext.BaseDirectory, exe));
        if (!Inside(full, AppContext.BaseDirectory) && !Inside(full, dataDir)) throw new UnauthorizedAccessException("String only runs programs from its own folders");
        if (!full.EndsWith(".exe", StringComparison.OrdinalIgnoreCase) || !File.Exists(full)) throw new FileNotFoundException("Program not found: " + exe);
        var psi = new ProcessStartInfo(full)
        {
            UseShellExecute = false, CreateNoWindow = true,
            RedirectStandardOutput = true, RedirectStandardError = true, RedirectStandardInput = true,
            StandardOutputEncoding = new UTF8Encoding(false), StandardErrorEncoding = new UTF8Encoding(false),
            WorkingDirectory = Path.GetDirectoryName(full)!,
        };
        psi.Environment["PYTHONIOENCODING"] = "utf-8";   // a Python program prints in any script
        psi.Environment["PYTHONUTF8"] = "1";
        if (a.TryGetProperty("args", out var args) && args.ValueKind == JsonValueKind.Array)
            foreach (var x in args.EnumerateArray()) psi.ArgumentList.Add(x.ValueKind == JsonValueKind.String ? x.GetString()! : x.GetRawText());
        var p = new Process { StartInfo = psi };
        var handle = "p" + ++procSeq;
        void Line(string stream, string? text)
        {
            if (text is null) return;
            Dispatcher.BeginInvoke(new Action(() => Post(new { kind = "proc", proc = handle, ev = "line", stream, text })));
        }
        p.OutputDataReceived += (_, e) => Line("out", e.Data);
        p.ErrorDataReceived += (_, e) => Line("err", e.Data);
        p.Start();
        procs[handle] = p;
        p.BeginOutputReadLine();
        p.BeginErrorReadLine();
        Try(() => p.StandardInput.Close());
        _ = Task.Run(() =>
        {
            p.WaitForExit();
            int code;
            try { code = p.ExitCode; } catch { code = -1; }
            Dispatcher.BeginInvoke(new Action(() => { procs.Remove(handle); Post(new { kind = "proc", proc = handle, ev = "exit", code }); p.Dispose(); }));
        });
        return handle;
    }

    void KillAll()
    {
        foreach (var p in procs.Values.ToList()) Try(() => p.Kill(true));
        foreach (var c in chats.Values.ToList()) Try(() => c.Cancel());
        if (ollamaServe is { HasExited: false }) Try(() => ollamaServe.Kill(true));   // only the one String started
    }

    /* ---------- Ollama: running already (its tray app), or started here for as long as String is open ---------- */
    async Task<bool> IsLocalModel(string name)
    {
        if (string.IsNullOrWhiteSpace(name) || Regex.IsMatch(name, @"(?:[:-]cloud|@cloud)$", RegexOptions.IgnoreCase)) return false;
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(10));
        using var query = new StringContent(JsonSerializer.Serialize(new { model = name }), Encoding.UTF8, "application/json");
        using var response = await ollamaHttp.PostAsync(ollamaBase + "/api/show", query, timeout.Token);
        if (!response.IsSuccessStatusCode) return false;
        var info = JsonNode.Parse(await response.Content.ReadAsStringAsync(timeout.Token));
        return string.IsNullOrEmpty((string?)info?["remote_host"]) && string.IsNullOrEmpty((string?)info?["remote_model"])
            && info?["model_info"] is JsonObject metadata && metadata.Count > 0;
    }

    async Task<bool> OllamaUp()
    {
        try
        {
            using var c = new CancellationTokenSource(TimeSpan.FromMilliseconds(1500));
            using var r = await http.GetAsync(ollamaBase + "/api/version", c.Token);
            return r.IsSuccessStatusCode;
        }
        catch { return false; }
    }
    async Task EnsureOllama()
    {
        if (await OllamaUp()) return;
        if (smoke) throw new IOException("Ollama isn't running");   // a self-test never starts the real one
        var exe = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Programs", "Ollama", "ollama.exe");
        if (!File.Exists(exe)) throw new FileNotFoundException("Ollama isn't installed (it's free at ollama.com)");
        if (ollamaServe is null || ollamaServe.HasExited)
            ollamaServe = Process.Start(new ProcessStartInfo(exe, "serve") { UseShellExecute = false, CreateNoWindow = true, WorkingDirectory = Path.GetDirectoryName(exe)! });
        for (int i = 0; i < 60; i++) { await Task.Delay(250); if (await OllamaUp()) return; }
        throw new IOException("Ollama didn't start");
    }

    /* ---------- closing: the page saves first ---------- */
    void OnClosing(object? sender, CancelEventArgs e)
    {
        if (closeNow) return;
        e.Cancel = true;
        if (closing) return;
        closing = true;
        SavePlacement();
        if (!loaded || web.CoreWebView2 is null) { CloseNow(); return; }
        flushAnswered = false;
        Post(new { kind = "flush" });   // the page saves, then answers "flushed"
        closeTimer?.Stop();
        closeTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(1500) };   // a page that doesn't answer doesn't keep String open
        closeTimer.Tick += (_, _) => { closeTimer?.Stop(); _ = FinishCloseAsync(); };
        closeTimer.Start();
    }

    // After the page's last saves: closes, unless one of them didn't reach the disk, or is still being written after
    // 10 s (a very slow disk). Then String stays open and the page says why, with "Close anyway". Saves that come in
    // while it waits (a late answer from the page, a change made meanwhile) are waited for too, and a page that answers
    // only after the wait began still gets asked. A page that never answered can't be asked: String closes regardless.
    async Task FinishCloseAsync()
    {
        closeTimer?.Stop();
        if (finishingClose || closeNow) return;
        finishingClose = true;
        var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(10);
        bool written = true;
        while (true)
        {
            var pending = storeChain;
            var left = deadline - DateTime.UtcNow;
            if (left <= TimeSpan.Zero || await Task.WhenAny(pending, Task.Delay(left)) != pending) { written = false; break; }
            await Task.Delay(60);   // lets a save (or the page's answer) that arrived meanwhile be queued
            if (storeChain == pending) break;   // nothing newer: everything asked for is written (or failed)
        }
        finishingClose = false;
        if (closeNow || !closing) return;
        if (flushAnswered && !closeAnyway && (!written || !saveErrors.IsEmpty) && (!smoke || smokeBreakSaves))
        {
            closing = false;
            Post(new { kind = "closeBlocked", error = !written ? "it's still writing them: the disk is very slow" : saveErrors.Values.FirstOrDefault() ?? "" });
            return;
        }
        CloseNow();
    }
    public void CloseNow()
    {
        if (closeNow) return;
        closeNow = true;
        Dispatcher.BeginInvoke(new Action(Close));   // Close() inside Closing throws, so do it after
    }

    public void BringToFront()
    {
        if (smoke) return;
        if (WindowState == WindowState.Minimized) WindowState = lastState;
        Show();
        Activate();
        Topmost = true;
        Topmost = false;
    }

    /* ---------- taskbar: previous, play/pause, next under String's thumbnail (hover its taskbar button) ---------- */
    ThumbButtonInfo? thumbPlay;
    ImageSource? glyphPlay, glyphPause;
    bool thumbPlaying;
    Action[] thumbActions = Array.Empty<Action>();

    void SetupThumbButtons()
    {
        // white glyphs on the dark taskbar; dark ones if Windows' taskbar is set to light
        bool light = false;
        try { using var k = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"); light = k?.GetValue("SystemUsesLightTheme") is int v && v == 1; } catch { }
        Brush ink = light ? new SolidColorBrush(Color.FromRgb(0x1a, 0x1a, 0x1c)) : new SolidColorBrush(Color.FromRgb(0xf2, 0xf2, 0xf4));
        ink.Freeze();
        ImageSource Glyph(string path)
        {
            var group = new DrawingGroup();
            group.Children.Add(new GeometryDrawing(Brushes.Transparent, null, new RectangleGeometry(new Rect(0, 0, 16, 16))));   // keeps the 16 px box
            group.Children.Add(new GeometryDrawing(ink, null, Geometry.Parse(path)));
            var img = new DrawingImage(group);
            img.Freeze();
            return img;
        }
        glyphPlay = Glyph("M4.5 2.5 L13.5 8 L4.5 13.5 Z");
        glyphPause = Glyph("M4 2.5 H7 V13.5 H4 Z M9 2.5 H12 V13.5 H9 Z");
        var prev = new ThumbButtonInfo { ImageSource = Glyph("M3 2.5 H5 V13.5 H3 Z M13.5 2.5 V13.5 L5.5 8 Z"), Description = "Previous" };
        thumbPlay = new ThumbButtonInfo { ImageSource = glyphPlay, Description = "Play" };
        var next = new ThumbButtonInfo { ImageSource = Glyph("M11 2.5 H13 V13.5 H11 Z M2.5 2.5 V13.5 L10.5 8 Z"), Description = "Next" };
        thumbActions = new Action[]
        {
            () => Post(new { kind = "media", action = "prev" }),
            () => Post(new { kind = "media", action = "toggle" }),
            () => Post(new { kind = "media", action = "next" }),
        };
        prev.Click += (_, _) => thumbActions[0]();
        thumbPlay.Click += (_, _) => thumbActions[1]();
        next.Click += (_, _) => thumbActions[2]();
        TaskbarItemInfo = new TaskbarItemInfo { ThumbButtonInfos = new ThumbButtonInfoCollection { prev, thumbPlay, next } };
    }

    // the page says whether music is playing: the middle button shows pause while it is
    void SetThumbPlaying(bool on)
    {
        thumbPlaying = on;
        if (thumbPlay is null) return;
        thumbPlay.ImageSource = on ? glyphPause : glyphPlay;
        thumbPlay.Description = on ? "Pause" : "Play";
    }

    /* ---------- window chrome ---------- */
    record Placement(double Left, double Top, double Width, double Height, bool Max);

    void RestorePlacement()
    {
        WindowStartupLocation = WindowStartupLocation.CenterScreen;
        try
        {
            var p = JsonSerializer.Deserialize<Placement>(File.ReadAllText(placementPath));
            if (p is null || p.Width < MinWidth || p.Height < MinHeight) return;
            var screen = new Rect(SystemParameters.VirtualScreenLeft, SystemParameters.VirtualScreenTop, SystemParameters.VirtualScreenWidth, SystemParameters.VirtualScreenHeight);
            if (!screen.Contains(new Point(p.Left + 80, p.Top + 16))) return;   // title bar must be reachable
            WindowStartupLocation = WindowStartupLocation.Manual;
            Left = p.Left; Top = p.Top; Width = p.Width; Height = p.Height;
            if (p.Max) WindowState = WindowState.Maximized;
        }
        catch { }
    }

    void SavePlacement()
    {
        if (smoke || !IsLoaded) return;
        try
        {
            var r = RestoreBounds;
            if (r.IsEmpty) return;
            File.WriteAllText(placementPath, JsonSerializer.Serialize(new Placement(r.Left, r.Top, r.Width, r.Height, WindowState == WindowState.Maximized)));
        }
        catch { }
    }

    [DllImport("dwmapi.dll")]
    static extern int DwmSetWindowAttribute(IntPtr hwnd, int attr, ref int value, int size);

    // where Navidrome keeps its database: DataFolder in its navidrome.ini (the MSI writes one), else its default
    string? NavidromeDb()
    {
        if (smoke) return navidromeDbArg;
        var dataFolder = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), "Navidrome");
        try
        {
            var ini = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "Navidrome", "navidrome.ini");
            if (File.Exists(ini))
                foreach (var line in File.ReadAllLines(ini))
                {
                    var m = Regex.Match(line, @"^\s*DataFolder\s*=\s*['""]?(.*?)['""]?\s*$", RegexOptions.IgnoreCase);
                    if (m.Success && m.Groups[1].Value.Length > 0) dataFolder = m.Groups[1].Value;
                }
        }
        catch { }
        return Path.Combine(dataFolder, "navidrome.db");
    }

    // the Recycle Bin, the way Explorer's Delete does it (undoable)
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct SHFILEOPSTRUCT
    {
        public IntPtr hwnd;
        public uint wFunc;
        [MarshalAs(UnmanagedType.LPWStr)] public string pFrom;
        [MarshalAs(UnmanagedType.LPWStr)] public string? pTo;
        public ushort fFlags;
        [MarshalAs(UnmanagedType.Bool)] public bool fAnyOperationsAborted;
        public IntPtr hNameMappings;
        [MarshalAs(UnmanagedType.LPWStr)] public string? lpszProgressTitle;
    }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    static extern int SHFileOperation(ref SHFILEOPSTRUCT op);
    static void Recycle(string path)
    {
        const uint FO_DELETE = 3;
        const ushort FOF_SILENT = 0x4, FOF_NOCONFIRMATION = 0x10, FOF_ALLOWUNDO = 0x40, FOF_NOERRORUI = 0x400, FOF_WANTNUKEWARNING = 0x4000;
        var op = new SHFILEOPSTRUCT { wFunc = FO_DELETE, pFrom = path + "\0\0", fFlags = FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI | FOF_WANTNUKEWARNING };
        int r = SHFileOperation(ref op);
        if (r != 0 || op.fAnyOperationsAborted) throw new IOException(op.fAnyOperationsAborted ? "Left where it was" : $"Windows couldn't move it to the Recycle Bin (error {r})");
        if (File.Exists(path)) throw new IOException("Windows didn't move it");
    }

    void StyleTitleBar()
    {
        var h = new WindowInteropHelper(this).Handle;
        int dark = 1, caption = 0x00060505, text = 0x00968e8e;   // COLORREF is 0x00BBGGRR: the page's own near-black
        DwmSetWindowAttribute(h, 20, ref dark, 4);
        DwmSetWindowAttribute(h, 35, ref caption, 4);
        DwmSetWindowAttribute(h, 34, ref caption, 4);
        DwmSetWindowAttribute(h, 36, ref text, 4);
    }

    static ImageSource? MakeIcon()
    {
        try { return BitmapFrame.Create(new Uri("pack://application:,,,/Assets/String.ico")); }
        catch { return null; }
    }

    /* ---------- --smoke-test: off-screen; the page's smokeTest() drives it; a screenshot each time its step changes ---------- */
    async Task RunSmokeAsync()
    {
        var outDir = smokeOut ?? Path.Combine(dataDir, "smoke");
        Directory.CreateDirectory(outDir);
        var core = web.CoreWebView2;
        async Task<string> Js(string code) => JsonSerializer.Deserialize<string>(await core.ExecuteScriptAsync($"(function(){{ const r = {code}; return typeof r === 'string' ? r : JSON.stringify(r); }})()")) ?? "";
        async Task Shot(string name, int wait)
        {
            await Task.Delay(wait);
            using var fs = File.Create(Path.Combine(outDir, name));
            await core.CapturePreviewAsync(CoreWebView2CapturePreviewImageFormat.Png, fs);
        }
        void Log(string k, string v) => smokeLog.Add($"{k,-14}{v}");
        var results = Path.Combine(outDir, "results.txt");
        try
        {
            await Task.Delay(1500);
            await Shot("01-open.png", 0);
            Log("test", await Js("window.smokeTest ? (window.smokeTest(), 'started') : 'no smokeTest()'"));
            string last = "", state = "";
            int n = 1;
            var until = DateTime.Now.AddSeconds(smokeTimeout);
            while (DateTime.Now < until)
            {
                await Task.Delay(400);
                state = await Js("JSON.stringify(window.smokeState || {})");
                using var sdoc = JsonDocument.Parse(state.Length > 0 && state[0] == '{' ? state : "{}");
                var step = sdoc.RootElement.TryGetProperty("step", out var sv) ? sv.ToString() : "";
                if (step.Length > 0 && step != last)
                {
                    last = step;
                    Log("step", step);
                    await Shot($"{++n:00}-{string.Concat(step.Split(Path.GetInvalidFileNameChars())).Replace(' ', '-')}.png", 250);
                }
                if (sdoc.RootElement.TryGetProperty("done", out var dv) && dv.ValueKind == JsonValueKind.True) break;
            }
            Log("final", state);
            await Shot($"{++n:00}-end.png", 300);
        }
        catch (Exception ex) { Log("error", ex.ToString()); }
        File.WriteAllLines(results, smokeLog);
        Close();
        await Task.Delay(4000);
        CloseNow();
    }
}
