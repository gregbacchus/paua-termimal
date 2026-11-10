//! `paua install` / `paua uninstall`: put the desktop entry and icons in the user's data
//! directory (`~/.local/share`) so GNOME lists Paua in the overview and the dock shows its icon.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::APP_ID;

/// The icon at each size, built into the binary.
pub(crate) const ICONS: [(u32, &[u8]); 7] = [
    (16, include_bytes!("../assets/icons/hicolor/16x16/apps/io.github.gregbacchus.Paua.png")),
    (32, include_bytes!("../assets/icons/hicolor/32x32/apps/io.github.gregbacchus.Paua.png")),
    (48, include_bytes!("../assets/icons/hicolor/48x48/apps/io.github.gregbacchus.Paua.png")),
    (64, include_bytes!("../assets/icons/hicolor/64x64/apps/io.github.gregbacchus.Paua.png")),
    (128, include_bytes!("../assets/icons/hicolor/128x128/apps/io.github.gregbacchus.Paua.png")),
    (256, include_bytes!("../assets/icons/hicolor/256x256/apps/io.github.gregbacchus.Paua.png")),
    (512, include_bytes!("../assets/icons/hicolor/512x512/apps/io.github.gregbacchus.Paua.png")),
];

/// A path as the `Exec` key wants it: quoted if it has anything unusual in it.
fn exec_quote(path: &Path) -> String {
    let s = path.display().to_string();
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
        return s;
    }
    let escaped: String = s
        .chars()
        .flat_map(|c| match c {
            '"' | '`' | '$' | '\\' => vec!['\\', '\\', c], // doubled backslash: the spec escapes twice
            '%' => vec!['%', '%'],
            c => vec![c],
        })
        .collect();
    format!("\"{escaped}\"")
}

pub fn desktop_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Paua\n\
         GenericName=Terminal\n\
         Comment=A colourful terminal\n\
         Exec={}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=System;TerminalEmulator;\n\
         Keywords=terminal;shell;console;command line;\n\
         StartupNotify=true\n\
         StartupWMClass={APP_ID}\n",
        exec_quote(exe)
    )
}

fn desktop_path(data: &Path) -> PathBuf {
    data.join("applications").join(format!("{APP_ID}.desktop"))
}

pub(crate) fn icon_path(data: &Path, size: u32) -> PathBuf {
    data.join("icons/hicolor").join(format!("{size}x{size}/apps/{APP_ID}.png"))
}

/// Write the files under `data` (an XDG data directory). Returns what it wrote.
pub fn install_into(data: &Path, exe: &Path) -> io::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    let desktop = desktop_path(data);
    fs::create_dir_all(desktop.parent().expect("has parent"))?;
    fs::write(&desktop, desktop_entry(exe))?;
    written.push(desktop);
    for (size, bytes) in ICONS {
        let p = icon_path(data, size);
        fs::create_dir_all(p.parent().expect("has parent"))?;
        fs::write(&p, bytes)?;
        written.push(p);
    }
    Ok(written)
}

/// Remove what `install_into` wrote. Returns what was there.
pub fn uninstall_from(data: &Path) -> Vec<PathBuf> {
    let mut paths = vec![desktop_path(data)];
    paths.extend(ICONS.iter().map(|(s, _)| icon_path(data, *s)));
    paths.into_iter().filter(|p| fs::remove_file(p).is_ok()).collect()
}

/// Ask the desktop to notice the change. Both tools are optional, so failures don't matter.
fn refresh_caches(data: &Path) {
    let quiet = |mut c: Command| {
        let _ = c.stdout(Stdio::null()).stderr(Stdio::null()).status();
    };
    let mut icons = Command::new("gtk-update-icon-cache");
    icons.args(["-q", "-f", "-t"]).arg(data.join("icons/hicolor"));
    quiet(icons);
    let mut apps = Command::new("update-desktop-database");
    apps.arg("-q").arg(data.join("applications"));
    quiet(apps);
}

/// Under Homebrew the binary sits in a versioned `Cellar` directory that changes on every upgrade;
/// the stable path is `<prefix>/bin/paua`. Anything else is used as it is.
fn stable_exe(exe: &Path) -> PathBuf {
    let parts: Vec<_> = exe.components().collect();
    if let (Some(i), Some(name)) = (parts.iter().position(|c| c.as_os_str() == "Cellar"), exe.file_name()) {
        let candidate = parts[..i].iter().collect::<PathBuf>().join("bin").join(name);
        if candidate.exists() {
            return candidate;
        }
    }
    exe.to_path_buf()
}

#[derive(Debug, Clone, Copy, Default)]
pub struct InstallOptions {
    /// Run the config wizard without asking (taking its suggestions) when there is no config yet.
    pub assume_yes: bool,
    /// Don't offer the config wizard at all.
    pub no_config: bool,
}

pub fn install(opts: InstallOptions) -> i32 {
    let exe = match std::env::current_exe().and_then(|p| p.canonicalize()) {
        Ok(p) => stable_exe(&p),
        Err(e) => {
            eprintln!("paua: can't find its own path: {e}");
            return 1;
        }
    };
    let data = gtk::glib::user_data_dir();
    match install_into(&data, &exe) {
        Ok(files) => {
            refresh_caches(&data);
            println!("Installed {} files under {}", files.len(), data.display());
            println!("  {}", desktop_path(&data).display());
            println!("Launches: {}", exe.display());
            if exe.components().any(|c| c.as_os_str() == "target") {
                println!(
                    "Note: that is a build directory. For a stable location use `cargo install --path .` \
                     and run `paua install` again."
                );
            }
            if !opts.no_config {
                crate::setup::offer_after_install(opts.assume_yes);
            }
            0
        }
        Err(e) => {
            eprintln!("paua: install failed: {e}");
            1
        }
    }
}

pub fn uninstall() -> i32 {
    let data = gtk::glib::user_data_dir();
    let removed = uninstall_from(&data);
    refresh_caches(&data);
    println!("Removed {} files", removed.len());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_paths_are_quoted_when_needed() {
        assert_eq!(exec_quote(Path::new("/home/me/.cargo/bin/paua")), "/home/me/.cargo/bin/paua");
        assert_eq!(exec_quote(Path::new("/opt/my apps/paua")), "\"/opt/my apps/paua\"");
        assert_eq!(exec_quote(Path::new("/opt/a$b/paua")), "\"/opt/a\\\\$b/paua\"");
    }

    #[test]
    fn homebrew_installs_use_the_stable_path() {
        let base = std::env::temp_dir().join(format!("paua-brew-test-{}", std::process::id()));
        let cellar = base.join("Cellar/paua/0.1.0/bin");
        fs::create_dir_all(&cellar).unwrap();
        fs::create_dir_all(base.join("bin")).unwrap();
        fs::write(cellar.join("paua"), "").unwrap();
        assert_eq!(stable_exe(&cellar.join("paua")), cellar.join("paua")); // no launcher yet
        fs::write(base.join("bin/paua"), "").unwrap();
        assert_eq!(stable_exe(&cellar.join("paua")), base.join("bin/paua"));
        assert_eq!(stable_exe(Path::new("/usr/bin/paua")), Path::new("/usr/bin/paua"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn desktop_entry_has_the_essentials() {
        let d = desktop_entry(Path::new("/usr/bin/paua"));
        assert!(d.starts_with("[Desktop Entry]\n"));
        for line in [
            "Name=Paua",
            "Exec=/usr/bin/paua",
            "Icon=io.github.gregbacchus.Paua",
            "StartupWMClass=io.github.gregbacchus.Paua",
            "Categories=System;TerminalEmulator;",
        ] {
            assert!(d.lines().any(|l| l == line), "missing {line}");
        }
    }

    #[test]
    fn installs_and_uninstalls_cleanly() {
        let dir = std::env::temp_dir().join(format!("paua-install-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let files = install_into(&dir, Path::new("/usr/bin/paua")).unwrap();
        assert_eq!(files.len(), 1 + ICONS.len());
        assert!(files.iter().all(|f| f.is_file()));
        let png = fs::read(icon_path(&dir, 256)).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert_eq!(uninstall_from(&dir).len(), files.len());
        assert!(uninstall_from(&dir).is_empty()); // nothing left the second time
        let _ = fs::remove_dir_all(&dir);
    }
}
