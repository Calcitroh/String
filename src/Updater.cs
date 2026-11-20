// String's updater. A newer release on GitHub is checked, downloaded, verified with String's release key, and then
// installed by that release's own installer (app\setup\install.ps1: the same one a person runs by hand). The installer
// closes String (which saves first), keeps the person's own changes and data, and opens the new String.
//   - Only an installed copy updates itself: setup\installed.txt (written by the installer) names this very folder. A
//     copy built from source never does.
//   - Only a zip signed with String's release key installs (ECDSA P-256 over SHA-256 of the whole zip, in
//     String.zip.sig). The private half of the key never leaves the publisher's PC.
//   - The release must say inside (app\setup\version.txt, covered by the signature) that it is the version it claims,
//     and newer than this one, so an older signed release can't be passed off as an update.
//   - Self-tests point it at a fake GitHub (--update-feed) with a key of their own (--update-key), and never start an
//     installer.
using System;
using System.Diagnostics;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Net.Http;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json.Nodes;
using System.Threading.Tasks;

namespace StringApp;

public sealed partial class MainWindow
{
    // the GitHub repository whose releases are String's ("owner/name")
    const string UpdateRepo = "Calcitroh/String";
    // String's release key, the public half (P-256, SubjectPublicKeyInfo, base64)
    const string UpdateKey = "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEc1dW08TlzhlcEqhsyFH7VHRAZn4I0I4O4NCTobsdY45jozA5DKGqz7GqVs5f4rtkBzstC78qXU0PM0RTwK3c3Q==";
    const long MaxRelease = 600L << 20;   // no String release is anywhere near this big

    string? updateFeedArg, updateKeyArg;   // self-tests only
    static readonly HttpClient updateHttp = MakeUpdateHttp();
    static HttpClient MakeUpdateHttp()
    {
        var h = new HttpClient { Timeout = TimeSpan.FromMinutes(30) };
        h.DefaultRequestHeaders.UserAgent.ParseAdd("String-updater/1.0");
        h.DefaultRequestHeaders.Accept.ParseAdd("application/vnd.github+json");
        return h;
    }

    static Version Three(Version v) => new(v.Major, v.Minor, Math.Max(0, v.Build));
    static Version CurrentVersion => Three(Assembly.GetExecutingAssembly().GetName().Version ?? new Version(0, 0, 0));
    string Feed => updateFeedArg ?? (UpdateRepo.Length > 0 ? $"https://api.github.com/repos/{UpdateRepo}/releases/latest" : "");

    // the folder this copy was installed into, or null for a copy built from source (or moved since it was installed)
    string? InstalledAt()
    {
        var here = Path.GetFullPath(AppContext.BaseDirectory).TrimEnd('\\');
        if (smoke) return updateFeedArg is null ? null : here;
        try
        {
            var mark = Path.Combine(here, "setup", "installed.txt");
            if (!File.Exists(mark)) return null;
            var to = Path.GetFullPath(File.ReadAllText(mark).Trim()).TrimEnd('\\');
            return string.Equals(to, here, StringComparison.OrdinalIgnoreCase) ? to : null;
        }
        catch { return null; }
    }

    sealed record Release(Version Version, string Notes, string ZipUrl, string SigUrl, long Size);

    // a reply read with a size limit, and a deadline for every read: a server that stops sending, or never stops,
    // can't keep the updater busy for ever (the signature check still guards what gets installed)
    static readonly TimeSpan Silence = TimeSpan.FromSeconds(60);
    async Task<byte[]> ReadLimited(HttpResponseMessage res, long max, Action<long, long>? progress = null, Stream? to = null, TimeSpan? within = null)
    {
        long total = res.Content.Headers.ContentLength ?? -1, got = 0;
        if (total > max) throw new InvalidDataException("That is far bigger than anything of String's: not downloaded");
        await using var src = await res.Content.ReadAsStreamAsync();
        var keep = to is null ? new MemoryStream() : null;
        var buf = new byte[1 << 16];
        var until = DateTime.UtcNow + (within ?? TimeSpan.FromMinutes(10));   // and a limit for the whole of it
        while (true)
        {
            var left = until - DateTime.UtcNow;
            if (left <= TimeSpan.Zero) throw new IOException("The download is far too slow: try again later");
            using var quiet = new System.Threading.CancellationTokenSource(left < Silence ? left : Silence);   // never past the whole limit
            int n;
            try { n = await src.ReadAsync(buf, quiet.Token); }
            catch (OperationCanceledException) { throw new IOException(DateTime.UtcNow >= until ? "The download is far too slow: try again later" : "The download stopped coming in: try again later"); }
            if (n == 0) break;
            got += n;
            if (got > max) throw new InvalidDataException("That is far bigger than anything of String's: stopped");
            if (to is not null) await to.WriteAsync(buf.AsMemory(0, n)); else keep!.Write(buf, 0, n);
            progress?.Invoke(got, total);
        }
        return keep?.ToArray() ?? Array.Empty<byte>();
    }
    async Task<HttpResponseMessage> Get(string url)
    {
        // only https, outside a self-test (whose fake GitHub is on this PC)
        if (!Uri.TryCreate(url, UriKind.Absolute, out var u) || (u.Scheme != "https" && !(smoke && u.IsLoopback)))
            throw new InvalidDataException("The release points somewhere String doesn't download from");
        using var deadline = new System.Threading.CancellationTokenSource(Silence);
        try { return await updateHttp.GetAsync(u, HttpCompletionOption.ResponseHeadersRead, deadline.Token); }
        catch (OperationCanceledException) { throw new IOException("GitHub didn't answer: try again later"); }
    }

    // the newest published release (drafts and pre-releases don't count), or null if there's none
    async Task<Release?> LatestRelease()
    {
        using var res = await Get(Feed);
        if ((int)res.StatusCode == 404) return null;   // nothing released yet
        res.EnsureSuccessStatusCode();
        var j = JsonNode.Parse(await ReadLimited(res, 2L << 20)) as JsonObject ?? throw new InvalidDataException("GitHub's answer wasn't a release");
        if ((bool?)j["draft"] == true || (bool?)j["prerelease"] == true) return null;
        var tag = (string?)j["tag_name"] ?? "";
        if (!Version.TryParse(tag.TrimStart('v', 'V'), out var v)) throw new InvalidDataException($"The newest release isn't named with a version number ({tag})");
        string zip = "", sig = "";
        long size = 0;
        foreach (var a in j["assets"] as JsonArray ?? new JsonArray())
        {
            var name = (string?)a?["name"];
            var url = (string?)a?["browser_download_url"] ?? "";
            if (name == "String.zip") { zip = url; size = (long?)a?["size"] ?? 0; }
            else if (name == "String.zip.sig") sig = url;
        }
        if (zip.Length == 0 || sig.Length == 0) throw new InvalidDataException("The newest release has no String.zip with its signature");
        var notes = ((string?)j["body"] ?? "").Trim();
        return new Release(Three(v), notes.Length > 4000 ? notes[..4000] + "…" : notes, zip, sig, size);
    }

    // what Settings shows without asking anyone: this version, and whether this copy can update itself
    object UpdateInfo() => new { current = CurrentVersion.ToString(3), installed = InstalledAt() is not null, published = Feed.Length > 0 };

    async Task<object> UpdateCheck()
    {
        var current = CurrentVersion.ToString(3);
        if (InstalledAt() is null) return new { current, installed = false };
        if (Feed.Length == 0) return new { current, installed = true, published = false };
        var r = await LatestRelease();
        return new { current, installed = true, published = true, latest = r?.Version.ToString(3), newer = r is not null && r.Version > CurrentVersion, notes = r?.Notes ?? "", size = r?.Size ?? 0 };
    }

    // downloads, verifies and unpacks the release named `want`, then starts its installer (a self-test only unpacks)
    async Task<object> UpdateInstall(string want)
    {
        var at = InstalledAt() ?? throw new InvalidOperationException("Only an installed String updates itself");
        if (Feed.Length == 0) throw new InvalidOperationException("This String has nowhere to get updates from");
        var r = await LatestRelease() ?? throw new InvalidOperationException("There's no release to install");
        var ver = r.Version.ToString(3);
        if (r.Version <= CurrentVersion) throw new InvalidOperationException("This String is already up to date");
        if (want.Length > 0 && want != ver) throw new InvalidOperationException("A newer release came out meanwhile: check again");
        if (r.Size > MaxRelease) throw new InvalidDataException("That release is far bigger than String: not downloaded");

        // <data>\updates: String's own folder, never through a link (earlier downloads in it go first)
        var dir = Path.Combine(dataDir, "updates");
        if (LinkInside(dataDir, dir)) throw new IOException("String's updates folder is a link to somewhere else: not used");
        if (Directory.Exists(dir)) Directory.Delete(dir, true);
        Directory.CreateDirectory(dir);
        var zipPath = Path.Combine(dir, $"String-{ver}.zip");
        var last = DateTime.MinValue;
        using (var res = await Get(r.ZipUrl))
        {
            res.EnsureSuccessStatusCode();
            await using var dst = File.Create(zipPath);
            await ReadLimited(res, MaxRelease, (got, total) =>
            {
                if (DateTime.UtcNow - last < TimeSpan.FromMilliseconds(250)) return;
                last = DateTime.UtcNow;
                Post(new { kind = "update", ev = "progress", got, total = total > 0 ? total : r.Size });
            }, dst, TimeSpan.FromHours(2));
        }
        Post(new { kind = "update", ev = "verifying" });
        byte[] sig;
        using (var res = await Get(r.SigUrl))
        {
            res.EnsureSuccessStatusCode();
            sig = Convert.FromBase64String(Encoding.ASCII.GetString(await ReadLimited(res, 4096)).Trim());
        }
        bool signed;
        using (var key = ECDsa.Create())
        {
            key.ImportSubjectPublicKeyInfo(Convert.FromBase64String(updateKeyArg ?? UpdateKey), out _);
            await using var z = File.OpenRead(zipPath);
            signed = key.VerifyData(z, sig, HashAlgorithmName.SHA256);
        }
        if (!signed)
        {
            try { File.Delete(zipPath); } catch { }
            throw new InvalidDataException("The download isn't signed with String's release key, so it wasn't installed");
        }
        var unpacked = Path.Combine(dir, $"String-{ver}");
        ZipFile.ExtractToDirectory(zipPath, unpacked);   // refuses entries that would land outside the folder
        try { File.Delete(zipPath); } catch { }
        var app = Path.Combine(unpacked, "app");
        var script = Path.Combine(app, "setup", "install.ps1");
        if (!File.Exists(script) || !File.Exists(Path.Combine(app, "String.exe"))) throw new InvalidDataException("The release is missing String's installer");
        var vf = Path.Combine(app, "setup", "version.txt");
        if (!File.Exists(vf) || !Version.TryParse(File.ReadAllText(vf).Trim(), out var inside) || Three(inside) != r.Version)
            throw new InvalidDataException("The release says inside that it's another version than its name: not installed");

        // Windows PowerShell runs the installer in a console window of its own (shown, so a problem can be read; it waits
        // for Enter if the installer fails), for the folder this String is installed in. The two paths go in as
        // environment variables, never as part of the command, so no character in them (quotes of any kind, %, &, ^,
        // letters of any script) can be read as code.
        var psi = new ProcessStartInfo("powershell.exe") { UseShellExecute = false, CreateNoWindow = false, WorkingDirectory = unpacked };
        foreach (var a in new[] { "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command",
                     "$Host.UI.RawUI.WindowTitle = 'Updating String'; & $env:STRING_UPDATE_SCRIPT -To $env:STRING_UPDATE_TO; if ($LASTEXITCODE) { Read-Host 'Press Enter to close' }" })
            psi.ArgumentList.Add(a);
        psi.Environment["STRING_UPDATE_SCRIPT"] = script;
        psi.Environment["STRING_UPDATE_TO"] = at;
        psi.Environment.Remove("PSModulePath");   // Windows PowerShell finds its own modules (see Install String.cmd)
        if (smoke) return new { ready = unpacked, version = ver, script = psi.Environment["STRING_UPDATE_SCRIPT"], to = psi.Environment["STRING_UPDATE_TO"] };
        Process.Start(psi);
        return new { started = true, version = ver };
    }
}