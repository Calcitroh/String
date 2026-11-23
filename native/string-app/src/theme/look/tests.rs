use super::*;

fn spec(css: &str) -> Spec {
    read(&parse(css))
}
const GOLD: [u8; 3] = [212, 175, 55];

/// The starter with every example switched on: the "/* --- …" line above each and the "*/" line below it gone, as
/// its own instructions say.
fn starter_on() -> String {
    STARTER
        .lines()
        .filter(|l| !l.trim_start().starts_with("/* ---") && l.trim() != "*/")
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_starter_as_made_is_no_look() {
    assert_eq!(spec(STARTER), Spec::default());
}

#[test]
fn the_starter_examples_mean_what_they_mean_in_1x() {
    let s = spec(&starter_on());
    // important, so over every tint
    for t in [Tint::Mono, Tint::Arc, Tint::Ember] {
        assert_eq!(s.tint_over(t), Some(GOLD), "{t:?}");
    }
    let b = &s.brand;
    assert_eq!(b.height, Some(96.));
    assert!(b.hide_wire && b.hide_name && !b.hidden);
    let logo = b.picture.as_ref().expect("the logo picture");
    assert_eq!(logo.url, "logo.png");
    assert_eq!(logo.fit, Fit::Contain);
    assert_eq!(logo.at, [Len::Pct(0.), Len::Pct(50.)]);
    assert_eq!(logo.repeat, [false, false]);
    assert!(logo.content && logo.clip_content);
    let main = s.main.as_ref().expect("the background");
    assert_eq!(main.url, "background.jpg");
    assert_eq!(main.fit, Fit::Cover);
    assert_eq!(main.at, [Len::Pct(50.), Len::Pct(50.)]);
    assert_eq!(main.repeat, [false, false]);
    assert!(!main.content && !main.clip_content);
}

#[test]
fn the_tint_follows_the_1x_cascade() {
    let over = |css: &str, t| spec(css).tint_over(t);
    // String's own Arc and Ember are more specific than a plain :root
    assert_eq!(over(":root { --hi: 1 2 3 }", Tint::Mono), Some([1, 2, 3]));
    assert_eq!(over(":root { --hi: 1 2 3 }", Tint::Arc), None);
    assert_eq!(over(":root { --hi: 1 2 3 }", Tint::Ember), None);
    assert_eq!(over(":root { --hi: 1 2 3 ! IMPORTANT }", Tint::Arc), Some([1, 2, 3]));
    assert_eq!(over(":root.tint-arc { --hi: 4 5 6 }", Tint::Arc), Some([4, 5, 6]));
    assert_eq!(over(":root.tint-arc { --hi: 4 5 6 }", Tint::Mono), None);
    assert_eq!(over(".tint-ember { --hi: 4 5 6 }", Tint::Ember), Some([4, 5, 6]));
    // html is less specific than String's own :root
    assert_eq!(over("html { --hi: 1 2 3 }", Tint::Mono), None);
    assert_eq!(over("html { --hi: 1 2 3 !important }", Tint::Mono), Some([1, 2, 3]));
    // what the body (or every element) says is what the page inherits
    assert_eq!(over("body { --hi: 7 8 9 }", Tint::Arc), Some([7, 8, 9]));
    assert_eq!(over("* { --hi: 7 8 9 }", Tint::Ember), Some([7, 8, 9]));
    assert_eq!(over(":root { --hi: 1 2 3 !important } body { --hi: 7 8 9 }", Tint::Mono), Some([7, 8, 9]));
    // later wins a tie; important wins over later
    assert_eq!(over(":root{--hi:1 2 3} :root{--hi:4 5 6}", Tint::Mono), Some([4, 5, 6]));
    assert_eq!(over(":root{--hi:1 2 3!important} :root{--hi:4 5 6}", Tint::Mono), Some([1, 2, 3]));
    // the numbers: commas too, clamped, percentages; anything else isn't a tint
    assert_eq!(over(":root { --hi: 300, -5, 50% }", Tint::Mono), Some([255, 0, 128]));
    for bad in ["#d4af37", "1 2", "1 2 3 4", "gold", "rgb(1 2 3)", "NaN 1 2", ""] {
        assert_eq!(over(&format!(":root {{ --hi: {bad} }}"), Tint::Mono), None, "{bad}");
    }
    // custom properties keep their case, and other places don't tint the page
    assert_eq!(over(":root { --HI: 1 2 3 }", Tint::Mono), None);
    assert_eq!(over(".brand { --hi: 1 2 3 }", Tint::Mono), None);
    assert_eq!(over(":root:hover { --hi: 1 2 3 }", Tint::Mono), None);
    assert_eq!(over(".string-hidden { --hi: 1 2 3 }", Tint::Mono), None);
}

#[test]
fn a_broken_file_is_read_as_far_as_it_goes() {
    for css in [
        "",
        "}}}{{{",
        "/* never closed :root { --hi: 1 2 3 }",
        "\u{feff}",
        ";;;:::",
        "@import url(other.css)",
        ":root { --hi: \"unclosed }",
        &String::from_utf8_lossy(&[0xff, 0, 0x7b, 0xc3, 0x28, 0x7d, 0x22, 0x5c]),
    ] {
        let _ = spec(css);
    }
    // the end of the file closes what's open
    assert_eq!(spec(":root { --hi: 1 2 3").tint_over(Tint::Mono), Some([1, 2, 3]));
    // at-rules are skipped whole, and what follows is still read
    let s = spec("@media (max-width: 900px) { :root { --hi: 1 2 3 !important } } @import 'x.css'; .brand { height: 10px }");
    assert_eq!(s.tint_over(Tint::Mono), None);
    assert_eq!(s.brand.height, Some(10.));
    // a stray brace spoils only the rule it's in
    let s = spec("} .brand { height: 20px } .brand { padding: 4px 6px }");
    assert_eq!(s.brand.height, None);
    assert_eq!(s.brand.padding, [Some(4.), Some(6.), Some(4.), Some(6.)]);
    // quotes and comments in values
    let s = spec("#main { background: url(\"a}/*b.png\") } :root { --hi: 1 /* x */ 2 3 }");
    assert_eq!(s.main.unwrap().url, "a}/*b.png");
    assert_eq!(s.tint_over(Tint::Mono), Some([1, 2, 3]));
    // nested rules are skipped with their selector
    let s = spec(".brand { .x { height: 5px } height: 30px; }");
    assert_eq!(s.brand.height, Some(30.));
}

#[test]
fn the_logo_box_reads_its_selectors() {
    let b = |css: &str| spec(css).brand;
    assert!(b("#side .brand > * { display: none }").hide_wire);
    assert!(b("aside  .brand >* { display: none }").hide_name);
    let only_name = b(".brand b { visibility: hidden }");
    assert!(only_name.hide_name && !only_name.hide_wire);
    let only_wire = b(".brand svg { opacity: 0 } ");
    assert!(only_wire.hide_wire && !only_wire.hide_name);
    assert!(b(".string-logo { display: none }").hide_wire);
    // a more specific rule shows the wire again
    let both = b(".brand > * { display: none } .brand svg { display: block }");
    assert!(both.hide_name && !both.hide_wire);
    // hover states, other places and a spoiled list don't count
    assert_eq!(b(".brand:hover { height: 9px } .side .brand { height: 9px }"), Brand::default());
    assert_eq!(b(".x, .brand::before { height: 9px }"), Brand::default());
    assert_eq!(b(".x, .brand { height: 9px }").height, Some(9.));
    assert!(b(".brand { display: none }").hidden);
    assert_eq!(b(".brand { height: 3em; padding: 1px 2px 3px 4px }").padding, [Some(1.), Some(2.), Some(3.), Some(4.)]);
    assert_eq!(b(".brand { height: 3em }").height, None);
}

#[test]
fn background_longhands_and_layers() {
    let main = |css: &str| spec(css).main;
    let m = main("#main { background-image: url('bg.png'); background-size: 50% auto; background-position: right bottom; background-repeat: repeat-x }").unwrap();
    assert_eq!(m.url, "bg.png");
    assert_eq!(m.fit, Fit::Sized(Some(Len::Pct(50.)), None));
    assert_eq!(m.at, [Len::Pct(100.), Len::Pct(100.)]);
    assert_eq!(m.repeat, [true, false]);
    // a longhand after the shorthand changes only its part; the shorthand after resets it
    let m = main("#main { background: url(a.png) no-repeat; background-size: contain }").unwrap();
    assert_eq!((m.fit, m.repeat), (Fit::Contain, [false, false]));
    let m = main("#main { background-size: contain; background: url(a.png) }").unwrap();
    assert_eq!((m.fit, m.repeat, m.at), (Fit::Sized(None, None), [true, true], [Len::Pct(0.), Len::Pct(0.)]));
    // the first layer with a picture, with its own size from each list
    let m = main("main { background: linear-gradient(#000, #111), url(x.png) top 10px / 20px 30px no-repeat #000 }").unwrap();
    assert_eq!(m.url, "x.png");
    assert_eq!(m.fit, Fit::Sized(Some(Len::Px(20.)), Some(Len::Px(30.))));
    assert_eq!(m.at[1], Len::Pct(0.));
    let m = main("#main { background-image: none, url(y.png); background-size: cover, contain }").unwrap();
    assert_eq!((m.url.as_str(), m.fit), ("y.png", Fit::Contain));
    assert!(main("#main { background: #123 }").is_none());
    assert!(main("#main { background: url() }").is_none());
    assert!(main("#mainx { background: url(a.png) }").is_none());
}

#[test]
fn only_pictures_inside_the_folder() {
    let dir = Path::new(r"C:\data\look");
    let ok = |u: &str| inside(dir, u);
    assert_eq!(ok("logo.png"), Some(dir.join("logo.png")));
    assert_eq!(ok("./pics/../pics/Logo.PNG?v=2#x"), Some(dir.join("pics").join("Logo.PNG")));
    assert_eq!(ok("my%20logo.webp"), Some(dir.join("my logo.webp")));
    assert_eq!(ok(r"pics\bg.jpg"), Some(dir.join("pics").join("bg.jpg")));
    for bad in [
        "../logo.png",
        "pics/../../logo.png",
        "%2e%2e/logo.png",
        "/logo.png",
        r"\\server\share\logo.png",
        r"C:\logo.png",
        "C:logo.png",
        "file:///C:/logo.png",
        "https://example.com/logo.png",
        "data:image/png;base64,AAAA",
        "logo.png:hidden",
        "logo.svg",
        "logo.avif",
        "look.css",
        "logo",
        "con.png",
        "pics/COM1.jpg",
        "%zz.png",
        "",
    ] {
        assert_eq!(ok(bad), None, "{bad}");
    }
}

/// A test folder of its own, with a look folder in it.
fn folder() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let look = t.path().join("look");
    fs::create_dir(&look).unwrap();
    (t, look)
}
fn png(path: &Path, w: u32, h: u32) {
    let mut im = RgbaImage::new(w, h);
    for (x, _, p) in im.enumerate_pixels_mut() {
        *p = image::Rgba([200, 200, 200, if x % 2 == 0 { 255 } else { 0 }]);
    }
    im.save(path).unwrap();
}

#[test]
fn a_look_loads_with_its_pictures() {
    let (_t, look) = folder();
    fs::write(
        look.join("look.css"),
        ":root { --hi: 212 175 55 !important; }\n.brand > * { display: none; }\n.brand { height: 96px; background: url(logo.png) left center / contain no-repeat content-box; }\n#main { background: url(pics/background.jpg) center / cover no-repeat; }",
    )
    .unwrap();
    png(&look.join("logo.png"), 40, 20);
    fs::create_dir(look.join("pics")).unwrap();
    image::RgbImage::new(64, 48).save(look.join("pics").join("background.jpg")).unwrap();
    let l = load(&look);
    assert_eq!(l.spec.tint_over(Tint::Arc), Some(GOLD));
    assert_eq!(l.logo.as_ref().map(|p| p.natural), Some([40, 20]));
    assert_eq!(l.main.as_ref().map(|p| p.natural), Some([64, 48]));
    assert!(l.changed().is_none());
    // a broken picture is no picture, and the rest of the look stays
    fs::write(look.join("logo.png"), b"not a picture").unwrap();
    let l = load(&look);
    assert!(l.logo.is_none() && l.main.is_some());
    assert_eq!(l.spec.tint_over(Tint::Mono), Some(GOLD));
    // a missing look.css, or a missing look folder, is no look
    fs::remove_file(look.join("look.css")).unwrap();
    let l = load(&look);
    assert_eq!(l.spec, Spec::default());
    assert!(l.logo.is_none() && l.main.is_none());
    assert_eq!(load(&look.join("nowhere")).spec, Spec::default());
}

#[test]
fn a_big_picture_is_kept_smaller() {
    let (_t, look) = folder();
    png(&look.join("big.png"), MAX_SIDE + 160, 10);
    fs::write(look.join("look.css"), "#main { background: url(big.png) }").unwrap();
    let pic = load(&look).main.unwrap();
    assert_eq!(pic.natural, [MAX_SIDE + 160, 10]);
    assert_eq!(pic.rgba.width(), MAX_SIDE);
}

#[test]
fn nothing_outside_the_folder() {
    let (t, look) = folder();
    png(&t.path().join("outside.png"), 8, 8);
    fs::write(
        look.join("look.css"),
        ".brand { background: url(../outside.png) } #main { background: url('%2E%2E/outside.png') }",
    )
    .unwrap();
    let l = load(&look);
    assert!(l.spec.brand.picture.is_some() && l.spec.main.is_some());
    assert!(l.logo.is_none() && l.main.is_none());
}

/// A junction (no special rights needed, unlike a symbolic link) from `link` to `to`.
fn junction(link: &Path, to: &Path) -> bool {
    std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(to)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn never_through_a_link() {
    let (t, look) = folder();
    let elsewhere = t.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    png(&elsewhere.join("logo.png"), 8, 8);
    fs::write(elsewhere.join("look.css"), ":root { --hi: 1 2 3 !important }").unwrap();
    assert!(junction(&look.join("linked"), &elsewhere), "couldn't make a junction");
    fs::write(look.join("look.css"), ".brand { background: url(linked/logo.png) }").unwrap();
    let l = load(&look);
    assert!(l.spec.brand.picture.is_some());
    assert!(l.logo.is_none(), "read through a junction");
    // the same picture, reached without the link, is fine
    fs::copy(elsewhere.join("logo.png"), look.join("logo.png")).unwrap();
    fs::write(look.join("look.css"), ".brand { background: url(logo.png) }").unwrap();
    assert!(load(&look).logo.is_some());
    // a look.css that is itself a link is no look (a symbolic link needs Developer Mode; skipped without it)
    fs::remove_file(look.join("look.css")).unwrap();
    if std::os::windows::fs::symlink_file(elsewhere.join("look.css"), look.join("look.css")).is_ok() {
        assert_eq!(load(&look).spec, Spec::default());
    }
}

#[test]
fn the_folder_is_made_once() {
    let (t, _) = folder();
    let dir = t.path().join("new").join("look");
    let (css, made) = make_folder(&dir).unwrap();
    assert!(made);
    assert_eq!(css, dir.join("look.css"));
    assert_eq!(fs::read_to_string(&css).unwrap(), STARTER);
    fs::write(&css, ":root { --hi: 1 2 3 }").unwrap();
    assert!(!make_folder(&dir).unwrap().1);
    assert_eq!(fs::read_to_string(&css).unwrap(), ":root { --hi: 1 2 3 }");
}

#[test]
fn a_saved_change_is_read_again_and_no_file_is_held() {
    let (_t, look) = folder();
    let css = look.join("look.css");
    fs::write(&css, ":root { --hi: 1 2 3 }").unwrap();
    let (tx, msgs) = unbounded();
    let (done, rx) = unbounded();
    let dir = look.clone();
    let w = std::thread::spawn(move || worker(dir, msgs, done, Context::default()));
    let next = || rx.recv_timeout(Duration::from_secs(5)).expect("the worker read the look");
    tx.send(Msg::On(true)).unwrap();
    assert_eq!(next().spec.tint_over(Tint::Mono), Some([1, 2, 3]));
    // saved the way careful editors save: a new file put in place of the old one, which fails if it's held open
    let tmp = look.join("look.css.tmp");
    fs::write(&tmp, ":root { --hi: 4 5 6 }").unwrap();
    fs::rename(&tmp, &css).unwrap();
    assert_eq!(next().spec.tint_over(Tint::Mono), Some([4, 5, 6]));
    // a picture appearing after the look names it
    fs::write(&css, "#main { background: url(bg.png) }").unwrap();
    assert!(next().main.is_none());
    png(&look.join("bg.png"), 4, 4);
    assert!(next().main.is_some());
    // deleted: no look; and the whole folder can go
    fs::remove_dir_all(&look).unwrap();
    assert_eq!(next().spec, Spec::default());
    // off: no more reading
    tx.send(Msg::On(false)).unwrap();
    fs::create_dir(&look).unwrap();
    fs::write(&css, ":root { --hi: 7 8 9 }").unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(900)).is_err());
    tx.send(Msg::On(true)).unwrap();
    assert_eq!(next().spec.tint_over(Tint::Mono), Some([7, 8, 9]));
    drop(tx);
    w.join().unwrap();
}
