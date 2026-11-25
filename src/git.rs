// Git lookups shared by both front-ends.

use std::fs;
use std::path::Path;

// HEAD of the repository containing `dir`: the nearest .git above it, either
// a directory or (worktrees, submodules) a file holding "gitdir: <path>".
pub(crate) fn find_head(dir: &str) -> Option<String> {
    let mut d = Path::new(dir);
    if !d.is_absolute() {
        return None;
    }
    loop {
        let git = d.join(".git");
        if let Ok(h) = fs::read_to_string(git.join("HEAD")) {
            return Some(h);
        }
        if let Ok(f) = fs::read_to_string(&git) {
            let gitdir = f.trim().strip_prefix("gitdir: ")?;
            return fs::read_to_string(d.join(gitdir).join("HEAD")).ok();
        }
        d = d.parent()?;
    }
}
