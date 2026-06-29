using System.Collections.ObjectModel;
using Terminal.Gui.App;
using Terminal.Gui.Views;
using Terminal.Gui.ViewBase;
using Terminal.Gui.Input;
using Terminal.Gui.Drawing;
using Wasmtime;

if (args.Length > 0 && args[0] == "interop")
{
    Interop.Run();
    return;
}
if (args.Length > 0 && args[0] == "__noop")
{
    Console.WriteLine("noop");
    return;
}

Tui.Run();

// ---------------------------------------------------------------------------
// R2 — native interop falsification (.NET side)
// ---------------------------------------------------------------------------
static class Interop
{
    public static void Run()
    {
        Console.WriteLine("== R2 native interop (.NET) ==");
        Wasm();
    }

    static void Wasm()
    {
        using var engine = new Engine();
        using var module = Module.FromText(
            engine, "add",
            "(module (func (export \"add\") (param i32 i32) (result i32) " +
            "local.get 0 local.get 1 i32.add))");
        using var store = new Store(engine);
        var instance = new Instance(store, module);
        var add = instance.GetFunction<int, int, int>("add")!;
        int result = add(20, 22);
        Console.WriteLine($"  wasm  : sandboxed tool add(20,22) = {result}  [Wasmtime]");
    }
}

// ---------------------------------------------------------------------------
// R1 — TUI render floor + ceiling (.NET side, Terminal.Gui v2)
// ---------------------------------------------------------------------------
static class Tui
{
    static bool _zoomedOut;
    static bool _drawerOpen = true;
    static bool _streaming;

    static readonly string[] FullLines =
    {
        "› refactor auth to use JWT instead of the legacy token check",
        "zoid  read auth.cs, api/tokens.cs (412 lines)",
        "      ● edited auth.cs        +12 -4",
        "      ● edited api/tokens.cs  +27 -9",
        "      ✓ 48 tests passed",
        "› now add rate limiting to the public endpoints",
    };
    static readonly string[] SummaryLines =
    {
        "› refactor auth to use JWT",
        "• read auth.cs, edited 2 files, tests pass",
        "› add rate limiting",
    };

    static ObservableCollection<string> _items = new(FullLines);

    public static void Run()
    {
        Application.Init();

        var win = new Window
        {
            Title = "zoid — CHAT  (q quit · z zoom · d drawer · Tab focus · s stream · click=select)",
            BorderStyle = LineStyle.Rounded,
        };

        var transcript = new FrameView
        {
            Title = "transcript",
            X = 0,
            Y = 0,
            Width = Dim.Fill()! - 36,
            Height = Dim.Fill(),
        };
        var list = new ListView
        {
            X = 0,
            Y = 0,
            Width = Dim.Fill(),
            Height = Dim.Fill(),
        };
        list.SetSource(_items);
        transcript.Add(list);

        var drawer = new FrameView
        {
            Title = "⑤ context",
            X = Pos.Right(transcript),
            Y = 0,
            Width = 36,
            Height = Dim.Fill(),
        };
        drawer.Add(new Label
        {
            Text = "● auth.cs       18k ███\n  tokens.cs     12k ██░\n  docs/auth.md   6k cold\n  system+tools  22k lock\n\n[x] evict cold → -6k",
        });

        win.Add(transcript, drawer);

        Application.KeyDown += (_, key) =>
        {
            if (key == Key.Q) { Application.RequestStop(); }
            else if (key == Key.Z)
            {
                _zoomedOut = !_zoomedOut;
                _items = new ObservableCollection<string>(_zoomedOut ? SummaryLines : FullLines);
                list.SetSource(_items);
            }
            else if (key == Key.D)
            {
                _drawerOpen = !_drawerOpen;
                drawer.Visible = _drawerOpen;
                transcript.Width = _drawerOpen ? Dim.Fill()! - 36 : Dim.Fill();
            }
            else if (key == Key.S) { StartStream(); }
        };

        Application.Run(win);
        win.Dispose();
        Application.Shutdown();
    }

    static void StartStream()
    {
        if (_streaming) return;
        _streaming = true;
        _items.Add("zoid  ");
        var idx = _items.Count - 1;
        var words = "I'll add a token-bucket limiter per IP, return 429 with Retry-After, and cover it with tests .".Split(' ');
        Task.Run(async () =>
        {
            foreach (var w in words)
            {
                await Task.Delay(90);
                Application.Invoke(() => { _items[idx] += w + " "; });
            }
            _streaming = false;
        });
    }
}
