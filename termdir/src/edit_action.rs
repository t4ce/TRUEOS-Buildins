//! Local desktop editing. Remote terminal handoff is deliberately separate.
use std::{io, path::Path};

pub fn is_local_ui4() -> bool {
    trueos::async_fs::block_on(trueos::async_fs::read_file(b"vFile:terminal-frontend"))
        .is_ok_and(|bytes| bytes == b"ui4")
}

fn start_script(path: &Path) -> io::Result<String> {
    let path = path.to_str().ok_or_else(|| io::Error::other("file path is not UTF-8"))?;
    if path.is_empty() || path.contains(['\n', '\r', '\0']) {
        return Err(io::Error::other("file path cannot be represented in an edit runscript"));
    }
    // The open directive takes the entire remainder as a literal path.
    Ok(format!("fs-scope trueosfs\nopen {path}\n"))
}

pub fn open(path: &Path) -> io::Result<()> {
    let script = start_script(path)?;
    trueos::vshell::launch_with_destination("edit", &script, trueos::vshell::LaunchDestination::NewShell3)
        .map_err(|code| io::Error::other(format!("host request code={code}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edit_script_preserves_the_selected_disc_and_literal_filename() {
        assert_eq!(start_script(Path::new("trueosfs:disc7/My notes/a; --name.txt")).unwrap(),
            "fs-scope trueosfs\nopen trueosfs:disc7/My notes/a; --name.txt\n");
        for path in ["", "note\nopen other.txt", "note\r.txt", "note\0.txt"] {
            assert!(start_script(Path::new(path)).is_err());
        }
    }
}
