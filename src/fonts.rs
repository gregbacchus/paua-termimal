//! Downloading Nerd Fonts: `paua fonts install ...`, and the offer in `paua setup`.
//!
//! Fonts come from the official Nerd Fonts release on GitHub, are checked against the SHA-256 sums
//! published with that release, and are installed with their licence files into
//! `~/.local/share/fonts/NerdFonts/<name>/`. Nothing is bundled with Paua: the user fetches the
//! fonts from their source, at their request.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::exec;

/// The Nerd Fonts release the downloads come from. Bump it to pick up newer fonts.
pub const NERD_FONTS_VERSION: &str = "v3.5.1";
const RELEASE_URL: &str = "https://github.com/ryanoasis/nerd-fonts/releases/download";
const PROJECT_URL: &str = "https://github.com/ryanoasis/nerd-fonts";

/// A downloadable font family.
pub struct Family {
    /// What you type: `paua fonts install VictorMono`.
    pub key: &'static str,
    /// The release asset (without `.tar.xz`).
    pub asset: &'static str,
    pub label: &'static str,
    /// Download size in MB (approximate, shown to the user).
    pub mb: f32,
    /// The family names it provides, for detecting that it is installed.
    pub provides: &'static str,
}

pub const CATALOG: &[Family] = &[
    Family { key: "VictorMono", asset: "VictorMono", label: "Victor Mono (ligatures, italics)", mb: 3.7, provides: "VictorMono Nerd Font" },
    Family { key: "Ubuntu", asset: "Ubuntu", label: "Ubuntu", mb: 2.8, provides: "Ubuntu Nerd Font" },
    Family { key: "UbuntuMono", asset: "UbuntuMono", label: "Ubuntu Mono", mb: 2.5, provides: "UbuntuMono Nerd Font" },
    Family { key: "UbuntuSans", asset: "UbuntuSans", label: "Ubuntu Sans", mb: 4.3, provides: "UbuntuSans Nerd Font" },
    Family { key: "JetBrainsMono", asset: "JetBrainsMono", label: "JetBrains Mono", mb: 7.3, provides: "JetBrainsMono Nerd Font" },
    Family { key: "FiraCode", asset: "FiraCode", label: "Fira Code", mb: 3.1, provides: "FiraCode Nerd Font" },
    Family { key: "CascadiaCode", asset: "CascadiaCode", label: "Cascadia Code", mb: 3.6, provides: "CaskaydiaCove Nerd Font" },
    Family { key: "Hack", asset: "Hack", label: "Hack", mb: 2.8, provides: "Hack Nerd Font" },
    Family { key: "Meslo", asset: "Meslo", label: "Meslo", mb: 5.2, provides: "MesloLGS Nerd Font" },
    Family { key: "SourceCodePro", asset: "SourceCodePro", label: "Source Code Pro", mb: 3.3, provides: "SauceCodePro Nerd Font" },
    Family { key: "IBMPlexMono", asset: "IBMPlexMono", label: "IBM Plex Mono", mb: 3.2, provides: "BlexMono Nerd Font" },
    Family { key: "RobotoMono", asset: "RobotoMono", label: "Roboto Mono", mb: 4.2, provides: "RobotoMono Nerd Font" },
    Family { key: "Symbols", asset: "NerdFontsSymbolsOnly", label: "Symbols only: icons for any other font", mb: 2.4, provides: "Symbols Nerd Font" },
];

/// What `paua setup` offers when it isn't installed: the terminal font, the chrome font and the
/// icons-only fallback.
pub const SETUP_OFFER: [&str; 3] = ["VictorMono", "Ubuntu", "Symbols"];

pub fn find(name: &str) -> Option<&'static Family> {
    CATALOG.iter().find(|f| f.key.eq_ignore_ascii_case(name) || f.asset.eq_ignore_ascii_case(name))
}

pub fn fonts_dir() -> PathBuf {
    gtk::glib::user_data_dir().join("fonts").join("NerdFonts")
}

/// Is this family installed (according to the list of installed font families)?
pub fn is_installed(f: &Family, families: &[String]) -> bool {
    families.iter().any(|x| x.starts_with(f.provides))
}

// ---------------------------------------------------------------------------------------------
// Download, verify, extract
// ---------------------------------------------------------------------------------------------

/// `SHA-256.txt`: lines of `<hex>  <file name>`.
pub fn parse_checksums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let mut p = l.split_whitespace();
            let (hash, name) = (p.next()?, p.next()?);
            (hash.len() == 64).then(|| (name.trim_start_matches('*').to_owned(), hash.to_ascii_lowercase()))
        })
        .collect()
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Fetch `url` into `dest` with curl, or wget if there is no curl.
fn download(url: &str, dest: &Path) -> Result<(), String> {
    let tools: [(&str, Vec<String>); 2] = [
        ("curl", ["-fsSL", "--retry", "2", "--connect-timeout", "20", "-o"].iter().map(|s| (*s).to_owned()).chain([dest.display().to_string(), url.to_owned()]).collect()),
        ("wget", ["-q", "-O"].iter().map(|s| (*s).to_owned()).chain([dest.display().to_string(), url.to_owned()]).collect()),
    ];
    let mut last = String::from("neither curl nor wget is installed");
    for (tool, args) in tools {
        let mut c = Command::new(tool);
        c.args(&args);
        match exec::run(c, Duration::from_secs(600)) {
            Ok(o) if o.success => return Ok(()),
            Ok(o) => last = format!("{tool}: {}", exec::first_line(&o.stderr)),
            Err(exec::ExecError::Spawn(_)) => {} // not installed: try the next one
            Err(e) => last = format!("{tool}: {e}"),
        }
    }
    Err(format!("couldn't download {url} ({last})"))
}

/// Unpack the fonts and licence files of `archive` (a `.tar.xz`) into `dest`. Returns how many font
/// files there were.
pub fn extract(archive: &Path, dest: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    // List first and pick the members ourselves: only plain top-level fonts and licence files.
    let mut list = Command::new("tar");
    list.arg("-tJf").arg(archive);
    let listing = match exec::run(list, Duration::from_secs(120)) {
        Ok(o) if o.success => o.stdout,
        Ok(o) => return Err(format!("tar: {}", exec::first_line(&o.stderr))),
        Err(e) => return Err(format!("tar: {e}")),
    };
    let wanted: Vec<&str> = listing
        .lines()
        .filter(|n| !n.is_empty() && !n.contains('/') && !n.starts_with('.'))
        .filter(|n| {
            let up = n.to_ascii_uppercase();
            n.ends_with(".ttf") || n.ends_with(".otf") || up.starts_with("LICEN") || up.starts_with("README") || up.starts_with("OFL")
        })
        .collect();
    if wanted.is_empty() {
        return Err("the archive held no font files".into());
    }
    let mut c = Command::new("tar");
    c.arg("-xJf").arg(archive).arg("-C").arg(dest).arg("--").args(&wanted);
    match exec::run(c, Duration::from_secs(120)) {
        Ok(o) if o.success => {}
        Ok(o) => return Err(format!("tar: {}", exec::first_line(&o.stderr))),
        Err(e) => return Err(format!("tar: {e}")),
    }
    let count = std::fs::read_dir(dest)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|e| matches!(e.path().extension().and_then(|x| x.to_str()), Some("ttf" | "otf")))
        .count();
    if count == 0 {
        return Err("the archive held no font files".into());
    }
    Ok(count)
}

/// Where downloads come from, so tests can substitute files.
pub trait Source {
    /// Fetch the file `name` of the release into `dest`.
    fn fetch(&self, name: &str, dest: &Path) -> Result<(), String>;
}

pub struct GitHub;

impl Source for GitHub {
    fn fetch(&self, name: &str, dest: &Path) -> Result<(), String> {
        download(&format!("{RELEASE_URL}/{NERD_FONTS_VERSION}/{name}"), dest)
    }
}

/// Download, verify and install `family` into `fonts_dir`. Progress goes to `out`.
pub fn install_family(
    family: &Family,
    source: &dyn Source,
    sums: &HashMap<String, String>,
    fonts_dir: &Path,
    scratch: &Path,
    out: &mut dyn Write,
) -> Result<usize, String> {
    let file = format!("{}.tar.xz", family.asset);
    let expected = sums.get(&file).ok_or_else(|| format!("{file} is not in the release's checksum list"))?;
    let archive = scratch.join(&file);
    let _ = writeln!(out, "  {} (~{} MB) ...", family.key, family.mb);
    source.fetch(&file, &archive)?;
    let actual = sha256_file(&archive).map_err(|e| e.to_string())?;
    if &actual != expected {
        let _ = std::fs::remove_file(&archive);
        return Err(format!("{file}: checksum mismatch (expected {expected}, got {actual}); not installed"));
    }
    let dest = fonts_dir.join(family.key);
    let _ = std::fs::remove_dir_all(&dest); // replace an older copy
    let n = extract(&archive, &dest)?;
    let _ = std::fs::remove_file(&archive);
    Ok(n)
}

/// Install the named families (keys from [`CATALOG`]) from the real release.
pub fn install_named(keys: &[&str], out: &mut dyn Write) -> Result<(), String> {
    install_named_with(keys, &GitHub, &fonts_dir(), out)
}

pub fn install_named_with(keys: &[&str], source: &dyn Source, fonts_dir: &Path, out: &mut dyn Write) -> Result<(), String> {
    let families: Vec<&Family> = keys
        .iter()
        .map(|k| find(k).ok_or_else(|| format!("unknown font {k:?} (see `paua fonts list`)")))
        .collect::<Result<_, _>>()?;
    static RUN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let run_id = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = gtk::glib::user_cache_dir().join("paua").join(format!("fonts-{}-{run_id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let run = || -> Result<(), String> {
        let _ = writeln!(out, "Downloading from the Nerd Fonts release {NERD_FONTS_VERSION} ({PROJECT_URL}) ...");
        let sums_file = scratch.join("SHA-256.txt");
        source.fetch("SHA-256.txt", &sums_file)?;
        let sums = parse_checksums(&std::fs::read_to_string(&sums_file).map_err(|e| e.to_string())?);
        for f in families {
            let n = install_family(f, source, &sums, fonts_dir, &scratch, out)?;
            let _ = writeln!(out, "    installed {n} font files into {}", fonts_dir.join(f.key).display());
        }
        Ok(())
    };
    let result = run();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// Ask fontconfig to notice new fonts (best effort).
pub fn refresh_font_cache(dir: &Path) {
    let mut c = Command::new("fc-cache");
    c.arg("-f").arg(dir);
    let _ = exec::run(c, Duration::from_secs(120));
}

pub const LICENCE_NOTE: &str = "The fonts are installed with their licence files (LICENCE.txt) next to them. Each font keeps its own\nlicence (for example Ubuntu: Ubuntu Font Licence; Victor Mono: SIL Open Font Licence), and the\nNerd Fonts icons have theirs: see https://github.com/ryanoasis/nerd-fonts#font-licensing.";

// ---------------------------------------------------------------------------------------------
// `paua fonts`
// ---------------------------------------------------------------------------------------------

pub const FONTS_USAGE: &str = "Usage: paua fonts <command>

  list                      the Nerd Fonts that can be downloaded, and which are installed
  install <name>...         download and install them (e.g. `paua fonts install VictorMono Ubuntu`)
  uninstall <name>...       remove fonts that `paua fonts install` installed
  Fonts come from the official Nerd Fonts release, are checked against its SHA-256 sums and go to
  ~/.local/share/fonts/NerdFonts. `paua setup` offers this for you.
";

pub fn cli(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("list") => {
            let families = crate::setup::installed_families();
            println!("Nerd Fonts available to download (release {NERD_FONTS_VERSION}):");
            for f in CATALOG {
                let mark = if is_installed(f, &families) { "installed" } else { "" };
                println!("  {:<14} {:<44} ~{:>4.1} MB  {mark}", f.key, f.label, f.mb);
            }
            println!("\nInstall with `paua fonts install <name>...`. Many more at https://www.nerdfonts.com/font-downloads.");
            0
        }
        Some("install") if args.len() > 1 => {
            let names: Vec<&str> = args[1..].iter().map(String::as_str).collect();
            let mut out = io::stdout();
            match install_named(&names, &mut out) {
                Ok(()) => {
                    refresh_font_cache(&fonts_dir());
                    println!("\n{LICENCE_NOTE}");
                    println!("Run `paua setup` to choose them.");
                    0
                }
                Err(e) => {
                    eprintln!("paua: {e}");
                    1
                }
            }
        }
        Some("uninstall") if args.len() > 1 => {
            let mut code = 0;
            for name in &args[1..] {
                match find(name) {
                    Some(f) if fonts_dir().join(f.key).is_dir() => {
                        match std::fs::remove_dir_all(fonts_dir().join(f.key)) {
                            Ok(()) => println!("Removed {}", f.key),
                            Err(e) => {
                                eprintln!("paua: {}: {e}", f.key);
                                code = 1;
                            }
                        }
                    }
                    Some(f) => println!("{} wasn't installed by Paua", f.key),
                    None => {
                        eprintln!("paua: unknown font {name:?}");
                        code = 1;
                    }
                }
            }
            refresh_font_cache(&fonts_dir());
            code
        }
        _ => {
            eprint!("{FONTS_USAGE}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake release: files made locally.
    struct Local(PathBuf);

    impl Source for Local {
        fn fetch(&self, name: &str, dest: &Path) -> Result<(), String> {
            std::fs::copy(self.0.join(name), dest).map(|_| ()).map_err(|e| format!("{name}: {e}"))
        }
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("paua-fonts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Build `Ubuntu.tar.xz` (with a font, a licence, and junk) and its SHA-256.txt.
    fn fake_release(dir: &Path, good_sum: bool) -> bool {
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("UbuntuNerdFont-Regular.ttf"), b"font").unwrap();
        std::fs::write(src.join("LICENCE.txt"), b"licence").unwrap();
        std::fs::write(src.join("junk.exe"), b"nope").unwrap();
        let made = Command::new("tar")
            .arg("-cJf")
            .arg(dir.join("Ubuntu.tar.xz"))
            .arg("-C")
            .arg(&src)
            .args(["UbuntuNerdFont-Regular.ttf", "LICENCE.txt", "junk.exe"])
            .status()
            .is_ok_and(|s| s.success());
        if !made {
            return false;
        }
        let sum = if good_sum { sha256_file(&dir.join("Ubuntu.tar.xz")).unwrap() } else { "0".repeat(64) };
        std::fs::write(dir.join("SHA-256.txt"), format!("{sum}  Ubuntu.tar.xz\n")).unwrap();
        true
    }

    #[test]
    fn checksum_lists_are_parsed() {
        let h = "a".repeat(64);
        let m = parse_checksums(&format!("{h}  Ubuntu.tar.xz\n{h} *Star.zip\nnot a line\nshort  x\n"));
        assert_eq!(m.len(), 2);
        assert_eq!(m["Ubuntu.tar.xz"], h);
        assert!(m.contains_key("Star.zip"));
    }

    #[test]
    fn sha256_matches_a_known_value() {
        let d = temp("sha");
        std::fs::write(d.join("f"), b"abc").unwrap();
        assert_eq!(sha256_file(&d.join("f")).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn catalog_is_consistent() {
        assert!(find("victormono").is_some() && find("NerdFontsSymbolsOnly").is_some() && find("nope").is_none());
        for k in SETUP_OFFER {
            assert!(find(k).is_some(), "{k}");
        }
        let mut keys: Vec<_> = CATALOG.iter().map(|f| f.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), CATALOG.len());
        let fams = vec!["Ubuntu Nerd Font Propo".to_owned(), "Hack Nerd Font Mono".to_owned()];
        assert!(is_installed(find("Ubuntu").unwrap(), &fams) && is_installed(find("Hack").unwrap(), &fams));
        assert!(!is_installed(find("VictorMono").unwrap(), &fams));
    }

    #[test]
    fn a_verified_download_is_installed_with_its_licence_and_nothing_else() {
        let d = temp("ok");
        if !fake_release(&d, true) {
            eprintln!("tar/xz not available: skipping");
            return;
        }
        let fonts = d.join("fonts");
        let mut out = Vec::new();
        install_named_with(&["Ubuntu"], &Local(d.clone()), &fonts, &mut out).unwrap();
        let dest = fonts.join("Ubuntu");
        assert!(dest.join("UbuntuNerdFont-Regular.ttf").is_file() && dest.join("LICENCE.txt").is_file());
        assert!(!dest.join("junk.exe").exists());
        assert!(String::from_utf8(out).unwrap().contains("installed 1 font files"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_checksum_mismatch_installs_nothing() {
        let d = temp("bad");
        if !fake_release(&d, false) {
            return;
        }
        let fonts = d.join("fonts");
        let err = install_named_with(&["Ubuntu"], &Local(d.clone()), &fonts, &mut Vec::new()).unwrap_err();
        assert!(err.contains("checksum mismatch"), "{err}");
        assert!(!fonts.join("Ubuntu").exists());
        // unknown names are rejected before any download
        assert!(install_named_with(&["Nope"], &Local(d.clone()), &fonts, &mut Vec::new()).unwrap_err().contains("unknown font"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
