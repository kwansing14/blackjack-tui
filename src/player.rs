use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

/// Who is sitting at this computer. The id is what the scoreboard keys on; it is made once,
/// kept in a file, and never shown. The name is what everyone else sees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
}

/// Longest name that still fits on a table line.
pub const MAX_NAME: usize = 16;

/// `$BLACKJACK_PROFILE`, else `$XDG_CONFIG_HOME/blackjack/player.json`, else
/// `$HOME/.config/blackjack/player.json`.
pub fn path() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("BLACKJACK_PROFILE") {
        return Ok(PathBuf::from(p));
    }
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME").ok_or("no HOME to keep a profile in; set BLACKJACK_PROFILE")?).join(".config"),
    };
    Ok(base.join("blackjack").join("player.json"))
}

/// Trimmed, 1 to MAX_NAME characters, nothing that would break a table line.
pub fn valid_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    let ok = !name.is_empty() && name.chars().count() <= MAX_NAME && !name.chars().any(char::is_control);
    ok.then(|| name.to_owned())
}

pub fn new_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}

impl Profile {
    /// The saved profile, or a new one. `BLACKJACK_NAME` always wins over the saved name.
    /// Without either, asks on stdin; this happens before the game takes stdin over.
    pub fn load_or_create() -> Result<Profile, String> {
        Self::load_or_create_with(&mut io::stdin().lock(), &mut io::stderr())
    }

    fn load_or_create_with(input: &mut impl BufRead, prompt_to: &mut impl Write) -> Result<Profile, String> {
        let env_name = std::env::var("BLACKJACK_NAME").ok().and_then(|n| valid_name(&n));
        let saved = Self::load()?;
        let profile = match (saved, env_name) {
            (Some(mut p), Some(name)) => {
                if p.name == name {
                    return Ok(p);
                }
                p.name = name;
                p
            }
            (Some(p), None) => return Ok(p),
            (None, Some(name)) => Profile { id: new_id(), name },
            (None, None) => Profile { id: new_id(), name: ask(input, prompt_to)? },
        };
        profile.save();
        Ok(profile)
    }

    /// The saved profile, if there is a readable one.
    pub fn load() -> Result<Option<Profile>, String> {
        let path = path()?;
        let Ok(text) = std::fs::read_to_string(&path) else { return Ok(None) };
        let p: Profile = serde_json::from_str(&text).map_err(|_| format!("{} is not a profile; delete it to start over", path.display()))?;
        Ok(valid_name(&p.name).map(|name| Profile { id: p.id, name }))
    }

    /// Writes the profile; a place we cannot write to costs a warning, not the game.
    pub fn save(&self) {
        let Ok(path) = path() else { return };
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&path, serde_json::to_string_pretty(self).unwrap_or_default() + "\n"));
        if let Err(e) = written {
            eprintln!("warning: could not save {} ({e}); scores for this run are not kept", path.display());
        }
    }
}

fn ask(input: &mut impl BufRead, prompt_to: &mut impl Write) -> Result<String, String> {
    let _ = write!(prompt_to, "your name? ");
    let _ = prompt_to.flush();
    let mut line = String::new();
    let _ = input.read_line(&mut line);
    valid_name(&line).ok_or_else(|| "no name given; set BLACKJACK_NAME or run: blackjack name <NAME>".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, MutexGuard};

    /// The env vars are process-wide, so tests that touch them take turns.
    static ENV: Mutex<()> = Mutex::new(());
    static SCRATCH: AtomicUsize = AtomicUsize::new(0);

    struct Sandbox {
        _lock: MutexGuard<'static, ()>,
        file: PathBuf,
    }

    /// A fresh profile path and a clear BLACKJACK_NAME, restored on drop.
    fn sandbox() -> Sandbox {
        let lock = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let n = SCRATCH.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("blackjack-profile-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("deeper").join("player.json");
        std::env::set_var("BLACKJACK_PROFILE", &file);
        std::env::remove_var("BLACKJACK_NAME");
        Sandbox { _lock: lock, file }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            std::env::remove_var("BLACKJACK_PROFILE");
            std::env::remove_var("BLACKJACK_NAME");
            if let Some(dir) = self.file.parent().and_then(|p| p.parent()) {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    fn create(typed: &str) -> Result<Profile, String> {
        let mut prompt = Vec::new();
        let r = Profile::load_or_create_with(&mut Cursor::new(typed), &mut prompt);
        assert!(prompt.is_empty() || prompt == b"your name? ", "{:?}", String::from_utf8_lossy(&prompt));
        r
    }

    #[test]
    fn names_are_trimmed_short_and_printable() {
        assert_eq!(valid_name("  alice \n"), Some("alice".into()));
        assert_eq!(valid_name("Bob the Dealer!"), Some("Bob the Dealer!".into()));
        assert_eq!(valid_name("sixteen chars ok"), Some("sixteen chars ok".into()));
        assert_eq!(valid_name(""), None);
        assert_eq!(valid_name("   "), None);
        assert_eq!(valid_name("seventeen chars!!"), None);
        assert_eq!(valid_name("tab\there"), None);
    }

    #[test]
    fn ids_are_32_hex_characters_and_never_repeat() {
        let (a, b) = (new_id(), new_id());
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn the_first_run_asks_for_a_name_and_saves_a_profile_with_a_new_id() {
        let sb = sandbox();
        let p = create("Alice\n").unwrap();
        assert_eq!(p.name, "Alice");
        assert_eq!(p.id.len(), 32);
        let on_disk: Profile = serde_json::from_str(&std::fs::read_to_string(&sb.file).unwrap()).unwrap();
        assert_eq!(on_disk, p, "saved as typed, with the directory made on the way");
        assert_eq!(create("Someone Else\n").unwrap(), p, "the next run does not ask again");
    }

    #[test]
    fn no_name_typed_is_an_error_and_nothing_is_saved() {
        let sb = sandbox();
        for typed in ["", "\n", "   \n"] {
            let err = create(typed).unwrap_err();
            assert!(err.starts_with("no name given"), "{err}");
        }
        assert!(!sb.file.exists());
    }

    #[test]
    fn blackjack_name_skips_the_prompt_and_renames_a_saved_profile_in_place() {
        let sb = sandbox();
        std::env::set_var("BLACKJACK_NAME", "bot-1");
        let p = create("").unwrap();
        assert_eq!(p.name, "bot-1");
        std::env::set_var("BLACKJACK_NAME", "bot-2");
        let renamed = create("").unwrap();
        assert_eq!((renamed.id.as_str(), renamed.name.as_str()), (p.id.as_str(), "bot-2"), "same person, new name");
        let on_disk: Profile = serde_json::from_str(&std::fs::read_to_string(&sb.file).unwrap()).unwrap();
        assert_eq!(on_disk, renamed);
    }

    #[test]
    fn a_saved_profile_with_a_bad_name_is_treated_as_missing_and_garbage_is_an_error() {
        let sb = sandbox();
        std::fs::create_dir_all(sb.file.parent().unwrap()).unwrap();
        std::fs::write(&sb.file, r#"{"id":"abc","name":""}"#).unwrap();
        assert_eq!(Profile::load().unwrap(), None);
        std::fs::write(&sb.file, "not json").unwrap();
        assert!(Profile::load().unwrap_err().contains("is not a profile"));
    }

    #[test]
    fn the_default_path_is_under_the_config_directory() {
        let _sb = sandbox();
        std::env::remove_var("BLACKJACK_PROFILE");
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/xdg");
        assert_eq!(path().unwrap(), PathBuf::from("/tmp/xdg/blackjack/player.json"));
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::set_var("HOME", "/home/someone");
        assert_eq!(path().unwrap(), PathBuf::from("/home/someone/.config/blackjack/player.json"));
        std::env::set_var("BLACKJACK_PROFILE", "/elsewhere/p.json");
        assert_eq!(path().unwrap(), PathBuf::from("/elsewhere/p.json"));
    }
}
