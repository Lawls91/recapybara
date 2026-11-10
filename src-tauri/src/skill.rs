//! The library skill for the user's own Claude sessions (D33).
use std::{fs, io, path::{Path, PathBuf}};

const TEMPLATE: &str = include_str!("../skill/SKILL.md");

/// The skill folder name is frozen (D50).
pub fn dir(home: &Path) -> PathBuf {
    home.join(".claude").join("skills").join("recapybara")
}

/// Writes `SKILL.md` for `library` when it is missing or differs, atomically, and nothing else. True when it wrote.
pub fn install(home: &Path, library: &str) -> io::Result<bool> {
    let want = TEMPLATE.replace("{{LIBRARY}}", library);
    let path = dir(home).join("SKILL.md");
    if fs::read(&path).is_ok_and(|have| have == want.as_bytes()) {
        return Ok(false);
    }
    fs::create_dir_all(dir(home))?;
    crate::library::write_atomic(&path, want.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mc-skill-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn skill(home: &Path) -> String {
        fs::read_to_string(home.join(r".claude\skills\recapybara\SKILL.md")).unwrap()
    }

    #[test]
    fn installs_the_library_path_and_layout_then_follows_a_new_path() {
        let home = temp("install");
        assert!(install(&home, r"D:\Meetings").unwrap());
        let s = skill(&home);
        assert!(s.starts_with("---\nname: recapybara\n") || s.starts_with("---\r\nname: recapybara\r\n"), "{s}");
        assert!(s.contains(r"`D:\Meetings`"), "{s}");
        assert!(!s.contains("{{LIBRARY}}"));
        for layout in ["`YYYY-MM-DD HHmm <Subject>`", "`meeting.json`", "`summary.md`", "`transcript.md`", "`notes.md`", "`glossary.md`"] {
            assert!(s.contains(layout), "layout lacks {layout}");
        }
        assert!(install(&home, r"E:\Moved library").unwrap());
        let s = skill(&home);
        assert!(s.contains(r"`E:\Moved library`") && !s.contains(r"D:\Meetings"), "{s}");
    }

    #[test]
    fn an_identical_installed_skill_is_not_rewritten() {
        let home = temp("same");
        install(&home, r"D:\Meetings").unwrap();
        let path = dir(&home).join("SKILL.md");
        let old = SystemTime::now() - Duration::from_secs(86_400);
        fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();

        assert!(!install(&home, r"D:\Meetings").unwrap());
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), old, "rewritten");

        // A differing copy (hand-edited, or an older app's) is replaced.
        fs::write(&path, "edited").unwrap();
        assert!(install(&home, r"D:\Meetings").unwrap());
        assert!(skill(&home).contains(r"`D:\Meetings`"));
    }

    /// Scope: the skill-install path only. D7's sweep of the app's own `runs` project folder is claude.rs's.
    #[test]
    fn writes_nothing_under_dot_claude_outside_its_skill_folder() {
        let home = temp("sandbox");
        let seeded = [r".claude\settings.json", r".claude\projects\other\keep.jsonl", r".claude\skills\other\SKILL.md", r".claude\skills\recapybara-old\SKILL.md"];
        for f in seeded {
            fs::create_dir_all(home.join(f).parent().unwrap()).unwrap();
            fs::write(home.join(f), f).unwrap();
        }
        fn files(dir: &Path, out: &mut Vec<String>, root: &Path) {
            for e in fs::read_dir(dir).unwrap().map(Result::unwrap) {
                if e.file_type().unwrap().is_dir() {
                    files(&e.path(), out, root);
                } else {
                    out.push(e.path().strip_prefix(root).unwrap().to_string_lossy().into_owned());
                }
            }
        }
        install(&home, r"D:\Meetings").unwrap();
        install(&home, r"E:\Moved").unwrap(); // a rewrite too
        install(&home, r"E:\Moved").unwrap();

        let mut found = vec![];
        files(&home, &mut found, &home);
        found.sort();
        let mut want: Vec<String> = seeded.iter().map(|s| s.to_string()).chain([r".claude\skills\recapybara\SKILL.md".into()]).collect();
        want.sort();
        assert_eq!(found, want, "no temp file left, nothing else written");
        for f in seeded {
            assert_eq!(fs::read_to_string(home.join(f)).unwrap(), f, "{f} changed");
        }
    }
}
