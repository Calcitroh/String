// String presence: shows what String is playing on Discord ("Listening to <song>", with a time bar), read from the
// presence.json String keeps in its data folder. String starts it while Discord is on in its settings and stops it when
// it closes (and it stops by itself if String goes away); Discord clears the status as soon as this program is gone.
//
//   string-presence <application id> <presence.json> [--pipe <name>] [--itunes <search url>] [--deezer <search url>]
//
// It prints one line per change, which String shows in its settings:
//   status waiting | status connected <name> | sent playing <title> | sent clear | warn <text> | error <text>
// Exit codes: 2 = bad arguments, 3 = Discord refused the application ID.
// --pipe, --itunes and --deezer are for tests (a fake Discord, fake cover searches); normally it tries discord-ipc-0..9,
// and for covers Apple's iTunes search, then Deezer's (only when String's "covers" setting is on, and then only the
// song and artist names go there).
using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;

static class Program
{
    static string clientId = "", file = "", pipeName = "", itunes = "https://itunes.apple.com/search", deezer = "https://api.deezer.com/search/track";
    static string lastStatus = "";
    static readonly object outLock = new();
    static readonly Dictionary<string, string> covers = new();     // "artist\ntitle" -> image URL, "" when none was found
    static readonly HashSet<string> looking = new();
    static string coverFile = "";
    static readonly HttpClient http = new() { Timeout = TimeSpan.FromSeconds(6) };

    static void Say(string line) { lock (outLock) { Console.WriteLine(line); Console.Out.Flush(); } }
    static void Status(string s) { if (s != lastStatus) { lastStatus = s; Say("status " + s); } }

    static async Task<int> Main(string[] args)
    {
        Console.OutputEncoding = new UTF8Encoding(false);
        var rest = new List<string>();
        for (int i = 0; i < args.Length; i++)
        {
            if (args[i] == "--pipe" && i + 1 < args.Length) pipeName = args[++i];
            else if (args[i] == "--itunes" && i + 1 < args.Length) itunes = args[++i];
            else if (args[i] == "--deezer" && i + 1 < args.Length) deezer = args[++i];
            else rest.Add(args[i]);
        }
        if (rest.Count < 2 || !Regex.IsMatch(rest[0], @"^\d{15,22}$"))
        {
            Say("error The application ID is the long number on your app's page on Discord's developer site");
            return 2;
        }
        clientId = rest[0];
        file = Path.GetFullPath(rest[1]);
        coverFile = Path.Combine(Path.GetDirectoryName(file)!, "discord-covers.json");
        LoadCovers();
        http.DefaultRequestHeaders.UserAgent.ParseAdd("String/1.0");

        // the String stops this program when it closes; if String itself goes away (crash, killed), stop too, so a
        // status never outlives it (String closes our stdin straight away, so that can't be the signal)
        WatchParent();

        while (true)
        {
            var pipe = await Connect();
            if (pipe is null) { Status("waiting"); await Task.Delay(10_000); continue; }
            try { await Session(pipe); }
            catch (BadIdException e) { Say("error " + e.Message); return 3; }
            catch (Exception e) when (e is IOException or ObjectDisposedException or OperationCanceledException or EndOfStreamException) { }
            catch (Exception e) { Say("warn " + e.Message); }
            finally { pipe.Dispose(); }
            Status("waiting");
            await Task.Delay(4_000);
        }
    }

    sealed class BadIdException(string m) : Exception(m);

    [StructLayout(LayoutKind.Sequential)]
    struct BasicInfo { public IntPtr ExitStatus, Peb, Affinity, Priority, Pid, ParentPid; }
    [DllImport("ntdll.dll")]
    static extern int NtQueryInformationProcess(IntPtr process, int infoClass, ref BasicInfo info, int size, out int returned);

    static void WatchParent()
    {
        int ppid = 0;
        using (var me = Process.GetCurrentProcess())
        {
            var bi = new BasicInfo();
            if (NtQueryInformationProcess(me.Handle, 0, ref bi, Marshal.SizeOf<BasicInfo>(), out _) == 0) ppid = (int)bi.ParentPid;
        }
        if (ppid <= 0) return;
        _ = Task.Run(async () =>
        {
            try
            {
                using var parent = Process.GetProcessById(ppid);
                using var me = Process.GetCurrentProcess();
                if (parent.StartTime > me.StartTime) return;   // that number now belongs to a newer program: can't tell, keep going
                await parent.WaitForExitAsync();
            }
            catch (ArgumentException) { }   // already gone
            catch { return; }
            Environment.Exit(0);
        });
    }

    static async Task<NamedPipeClientStream?> Connect()
    {
        var names = pipeName.Length > 0 ? new[] { pipeName } : Enumerable.Range(0, 10).Select(i => "discord-ipc-" + i).ToArray();
        foreach (var n in names)
        {
            var p = new NamedPipeClientStream(".", n, PipeDirection.InOut, PipeOptions.Asynchronous);
            try { await p.ConnectAsync(400); return p; }
            catch { p.Dispose(); }
        }
        return null;
    }

    /* ---------- frames: opcode and length (little-endian uint32), then JSON ---------- */
    static readonly SemaphoreSlim writeLock = new(1, 1);
    static async Task Send(Stream s, int op, string json)
    {
        var body = Encoding.UTF8.GetBytes(json);
        var buf = new byte[8 + body.Length];
        BitConverter.TryWriteBytes(buf.AsSpan(0, 4), (uint)op);
        BitConverter.TryWriteBytes(buf.AsSpan(4, 4), (uint)body.Length);
        body.CopyTo(buf, 8);
        await writeLock.WaitAsync();
        try { await s.WriteAsync(buf); await s.FlushAsync(); }
        finally { writeLock.Release(); }
    }
    static async Task<(int op, JsonNode? json, string raw)> Read(Stream s, CancellationToken ct)
    {
        var head = new byte[8];
        await s.ReadExactlyAsync(head, ct);
        int op = (int)BitConverter.ToUInt32(head, 0), len = (int)BitConverter.ToUInt32(head, 4);
        if (len < 0 || len > (1 << 20)) throw new IOException("frame too large");
        var body = new byte[len];
        await s.ReadExactlyAsync(body, ct);
        var raw = Encoding.UTF8.GetString(body);
        JsonNode? j = null;
        try { j = JsonNode.Parse(raw); } catch { }
        return (op, j, raw);
    }

    static async Task Session(NamedPipeClientStream pipe)
    {
        using var cts = new CancellationTokenSource();
        await Send(pipe, 0, new JsonObject { ["v"] = 1, ["client_id"] = clientId }.ToJsonString());
        // a Discord that takes the connection but never answers it isn't waited on for ever: tried again later
        using var hello = CancellationTokenSource.CreateLinkedTokenSource(cts.Token);
        hello.CancelAfter(TimeSpan.FromSeconds(10));
        var (op, j, _) = await Read(pipe, hello.Token);
        if (op == 2)
        {
            int code = (int?)j?["code"] ?? 0;
            var msg = (string?)j?["message"] ?? "closed";
            if (code == 4000 || msg.Contains("client", StringComparison.OrdinalIgnoreCase)) throw new BadIdException("Discord doesn't know that application ID: check it on Discord's developer site");
            throw new IOException(msg);
        }
        if ((string?)j?["evt"] != "READY") throw new IOException("Discord didn't say READY");
        var user = j?["data"]?["user"];
        Status("connected " + ((string?)user?["global_name"] ?? (string?)user?["username"] ?? "to Discord"));

        // Discord's side: answer pings, notice errors and the pipe closing
        var reader = Task.Run(async () =>
        {
            try
            {
                while (!cts.IsCancellationRequested)
                {
                    var (o, msg, raw) = await Read(pipe, cts.Token);
                    if (o == 3) await Send(pipe, 4, raw);
                    else if (o == 2) break;
                    else if ((string?)msg?["evt"] == "ERROR") Say("warn " + ((string?)msg?["data"]?["message"] ?? "Discord said no"));
                }
            }
            catch { }
            cts.Cancel();
        });

        // our side: follow presence.json; at most one update every 4.2 s (Discord allows 5 per 20 s), always the newest
        string sent = "\u0000", sentSummary = "";
        DateTime lastSend = DateTime.MinValue, seen = DateTime.MinValue;
        long seenLen = -1;
        JsonObject? doc = null;
        while (!cts.IsCancellationRequested)
        {
            try
            {
                var fi = new FileInfo(file);
                if (fi.Exists && (fi.LastWriteTimeUtc != seen || fi.Length != seenLen))
                {
                    var text = await File.ReadAllTextAsync(file, cts.Token);
                    doc = JsonNode.Parse(text) as JsonObject;
                    seen = fi.LastWriteTimeUtc;
                    seenLen = fi.Length;
                }
            }
            catch (OperationCanceledException) { break; }
            catch { }   // caught mid-write: read it again next time
            var (activity, summary) = Build(doc);
            if (activity != sent && DateTime.UtcNow - lastSend >= TimeSpan.FromMilliseconds(4200))
            {
                var args = new JsonObject { ["pid"] = Environment.ProcessId };
                if (activity.Length > 0) args["activity"] = JsonNode.Parse(activity);
                await Send(pipe, 1, new JsonObject { ["cmd"] = "SET_ACTIVITY", ["args"] = args, ["nonce"] = Guid.NewGuid().ToString() }.ToJsonString());
                sent = activity;
                lastSend = DateTime.UtcNow;
                if (summary != sentSummary) { sentSummary = summary; Say("sent " + summary); }
            }
            try { await Task.Delay(400, cts.Token); } catch { break; }
        }
        await reader;
    }

    /* ---------- what Discord is told: "" clears the status ---------- */
    static (string activity, string summary) Build(JsonObject? d)
    {
        if (d is null || (string?)d["state"] != "playing") return ("", "clear");
        long now = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        long start = Num(d["startedAt"]), end = Num(d["endsAt"]);
        if (end > 0 && end < now - 3000) return ("", "clear");   // left over from a session that ended without saying so
        var title = Fit((string?)d["title"] ?? "");
        if (title.Length == 0) return ("", "clear");
        var artist = Fit((string?)d["artist"] ?? "");
        var album = Fit((string?)d["album"] ?? "");
        var a = new JsonObject
        {
            ["type"] = 2,   // Listening
            ["status_display_type"] = (string?)d["display"] switch { "app" => 0, "artist" => 1, _ => 2 },
            ["details"] = title,
        };
        if (artist.Length > 0) a["state"] = artist;
        if (start > 0 && end > start) a["timestamps"] = new JsonObject { ["start"] = start, ["end"] = end };
        var assets = new JsonObject();
        var cover = (bool?)d["covers"] == true && artist.Length > 0 ? Cover((string?)d["artist"] ?? "", (string?)d["title"] ?? "") : "";
        if (cover.Length > 0)
        {
            assets["large_image"] = cover;
            assets["large_text"] = album.Length > 0 ? album : title;
        }
        if (assets.Count > 0) a["assets"] = assets;
        return (a.ToJsonString(), "playing " + title);
    }
    static long Num(JsonNode? n) { try { return n is null ? 0 : (long)n.GetValue<double>(); } catch { return 0; } }
    // Discord wants 2 to 128 characters
    static string Fit(string s)
    {
        s = s.Trim();
        if (s.Length == 0) return "";
        if (s.Length < 2) s += "⠀";
        if (s.Length > 128) s = s[..127] + "…";
        return s;
    }

    /* ---------- covers: Apple's iTunes search, then Deezer's, only when covers are turned on; remembered in
       discord-covers.json ("v2" keys: a song Apple alone didn't know is asked again now that Deezer is asked too) ---------- */
    static string Cover(string artist, string title)
    {
        var key = "v2\n" + artist.Trim().ToLowerInvariant() + "\n" + title.Trim().ToLowerInvariant();
        lock (covers)
        {
            if (covers.TryGetValue(key, out var url)) return url;
            if (!looking.Add(key)) return "";
        }
        _ = Task.Run(async () =>
        {
            string N(string? s) => Regex.Replace((s ?? "").ToLowerInvariant(), @"[^\p{L}\p{N}]+", " ").Trim();
            string na = N(artist), nt = N(title);
            // one name against the search's: the same once punctuation and case are set aside, or one inside the other
            // ("Remastered" and the like on either side are fine). A name of symbols only ("!!!") has nothing left
            // after that, and would match anything: then the search's name has to be the same symbols.
            bool Match(string? theirs, string mine, string nmine)
            {
                if (nmine.Length == 0) return mine.Trim().Length > 0 && string.Equals((theirs ?? "").Trim(), mine.Trim(), StringComparison.OrdinalIgnoreCase);
                var r = N(theirs);
                return r.Length > 0 && (r.Contains(nmine) || nmine.Contains(r));
            }
            bool Same(string? artistName, string? trackName) => Match(artistName, artist, na) && Match(trackName, title, nt);
            string found = "";
            bool appleAnswered = false, deezerAnswered = false;
            try
            {
                var j = JsonNode.Parse(await http.GetStringAsync($"{itunes}?media=music&entity=song&limit=5&term={Uri.EscapeDataString(artist + " " + title)}"));
                appleAnswered = true;
                foreach (var r in j?["results"]?.AsArray() ?? new JsonArray())
                {
                    var art = (string?)r?["artworkUrl100"];
                    if (Same((string?)r?["artistName"], (string?)r?["trackName"]) && art is not null && art.StartsWith("https://")) { found = art.Replace("100x100bb", "600x600bb"); break; }
                }
            }
            catch { }
            if (found.Length == 0)
            {
                try
                {
                    // Deezer: exact first (artist:"…" track:"…"), then loose words
                    foreach (var q in new[] { $"artist:\"{artist}\" track:\"{title}\"", artist + " " + title })
                    {
                        var j = JsonNode.Parse(await http.GetStringAsync($"{deezer}?limit=5&q={Uri.EscapeDataString(q)}"));
                        deezerAnswered = true;
                        foreach (var r in j?["data"]?.AsArray() ?? new JsonArray())
                        {
                            var art = (string?)r?["album"]?["cover_xl"] ?? (string?)r?["album"]?["cover_big"];
                            if (Same((string?)r?["artist"]?["name"], (string?)r?["title"]) && art is not null && art.StartsWith("https://")) { found = art; break; }
                        }
                        if (found.Length > 0) break;
                    }
                }
                catch { }
            }
            string? json = null;
            long ver = 0;
            lock (covers)
            {
                looking.Remove(key);
                // remembered when found, or when both said no; a lookup that failed (offline) is tried again next time
                if (found.Length > 0 || (appleAnswered && deezerAnswered)) { covers[key] = found; json = CoversJson(); ver = ++coversVer; }
            }
            if (json is not null) SaveCovers(json, ver);   // written after letting go of the covers, so a slow disk holds up nothing
        });
        return "";
    }
    static void LoadCovers()
    {
        try
        {
            if (JsonNode.Parse(File.ReadAllText(coverFile)) is JsonObject o)
                foreach (var kv in o) covers[kv.Key] = (string?)kv.Value ?? "";
        }
        catch { }
    }
    // the newest 3000 (call while holding the covers lock)
    static string CoversJson()
    {
        var o = new JsonObject();
        foreach (var kv in covers.TakeLast(3000)) o[kv.Key] = kv.Value;
        return o.ToJsonString();
    }
    static readonly object saveLock = new();
    static long coversVer, savedVer;
    static void SaveCovers(string json, long ver)
    {
        lock (saveLock)   // one write at a time (never an older list over a newer one); the covers stay free meanwhile
        {
            if (ver <= savedVer) return;
            try
            {
                var tmp = coverFile + ".tmp";
                File.WriteAllText(tmp, json);
                File.Move(tmp, coverFile, true);
                savedVer = ver;
            }
            catch { }
        }
    }
}
