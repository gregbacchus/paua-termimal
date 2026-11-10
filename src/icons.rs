//! Icons built into the binary.
//!
//! The split / pop-out icons are Paua's own symbolic icons, and the app icon is looked up by name.
//! A development build finds them in `./assets/icons`, but an installed binary (a release tarball,
//! Homebrew) has no such folder, so they are written to the user's cache directory at start-up and
//! that directory is added to the icon theme's search path.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::APP_ID;

/// Paua's own symbolic action icons (`hicolor/scalable/actions/<name>.svg`).
const ACTION_ICONS: [(&str, &[u8]); 4] = [
    ("pop-out-pip-symbolic", include_bytes!("../assets/icons/hicolor/scalable/actions/pop-out-pip-symbolic.svg")),
    ("pop-out-symbolic", include_bytes!("../assets/icons/hicolor/scalable/actions/pop-out-symbolic.svg")),
    ("split-down-symbolic", include_bytes!("../assets/icons/hicolor/scalable/actions/split-down-symbolic.svg")),
    ("split-right-symbolic", include_bytes!("../assets/icons/hicolor/scalable/actions/split-right-symbolic.svg")),
];

/// Where the icons are written (an icon theme search path).
pub fn cache_dir() -> PathBuf {
    gtk::glib::user_cache_dir().join("paua").join("icons")
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    if fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // Write to a side file and rename, so another Paua starting at the same time never reads half a file.
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(true)
}

/// Write every built-in icon under `base` (`base/hicolor/...`). Returns how many files changed.
pub fn write_icons(base: &Path) -> io::Result<usize> {
    let mut changed = 0;
    for (name, bytes) in ACTION_ICONS {
        let p = base.join("hicolor/scalable/actions").join(format!("{name}.svg"));
        changed += usize::from(write_if_changed(&p, bytes)?);
    }
    for (size, bytes) in crate::install::ICONS {
        let p = base.join("hicolor").join(format!("{size}x{size}/apps/{APP_ID}.png"));
        changed += usize::from(write_if_changed(&p, bytes)?);
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_written_once_and_repaired_when_changed() {
        let dir = std::env::temp_dir().join(format!("paua-icons-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let first = write_icons(&dir).unwrap();
        assert_eq!(first, ACTION_ICONS.len() + crate::install::ICONS.len());
        assert!(dir.join("hicolor/scalable/actions/split-right-symbolic.svg").is_file());
        assert!(dir.join(format!("hicolor/256x256/apps/{APP_ID}.png")).is_file());
        assert_eq!(write_icons(&dir).unwrap(), 0); // nothing to do the second time
        fs::write(dir.join("hicolor/scalable/actions/pop-out-symbolic.svg"), b"broken").unwrap();
        assert_eq!(write_icons(&dir).unwrap(), 1); // a damaged file is rewritten
        let _ = fs::remove_dir_all(&dir);
    }
}
