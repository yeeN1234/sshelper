//! Reading and updating `~/.ssh/config`.
//!
//! Only `Host` / `Match` headers are interpreted; everything else is kept
//! verbatim. When a host entry is added:
//!
//! * an existing single-alias `Host <alias>` section is replaced in place;
//! * otherwise the entry goes right before the first `Host *` section — ssh
//!   uses the *first* value it reads, so an entry below `Host *` would have
//!   its `User` etc. overridden;
//! * otherwise it is appended.
//!
//! Files written by Windows tools with a UTF-8 BOM or as UTF-16 (e.g.
//! `echo ... > config` in Windows PowerShell) break ssh; they are decoded and
//! written back as UTF-8 without BOM.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    Host,
    Match,
}

/// A `Host` or `Match` section. `start` is the header line; `end` is the last
/// line that belongs to it — trailing blank lines and comments are left out
/// because they usually describe the next section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub keyword: Keyword,
    pub start: usize,
    pub end: usize,
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeAction {
    Replaced,
    InsertedBeforeWildcard,
    Appended,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("別名「{alias}」位於多別名的區塊 `Host {patterns}`，請換一個別名或手動編輯 config")]
    AliasInMultiHost { alias: String, patterns: String },
    #[error("config 已有 Host {0}，未允許覆蓋")]
    AliasExists(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// The fields sshelper writes for a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEntry {
    pub alias: String,
    pub host_name: String,
    pub user: String,
    pub port: u16,
    pub identity_file: String,
}

impl HostEntry {
    /// The section lines; `stamp` goes into a comment (usually today's date).
    pub fn to_lines(&self, stamp: &str) -> Vec<String> {
        let mut lines = vec![
            format!("Host {}", self.alias),
            format!("    # added by sshelper {stamp}"),
            format!("    HostName {}", self.host_name),
            format!("    User {}", quote(&self.user)),
        ];
        if self.port != 22 {
            lines.push(format!("    Port {}", self.port));
        }
        lines.push(format!("    IdentityFile {}", quote(&self.identity_file)));
        lines.push("    IdentitiesOnly yes".to_owned());
        lines
    }
}

fn quote(value: &str) -> String {
    if value.contains(char::is_whitespace) {
        format!("\"{value}\"")
    } else {
        value.to_owned()
    }
}

/// `IdentityFile` value for a private key: `~/.ssh/<name>` when the key lives
/// in the user's real `~/.ssh` (portable, no spaces from the profile path),
/// otherwise the absolute path with forward slashes.
pub fn identity_file_value(paths: &crate::SshPaths, private_key: &Path) -> String {
    if paths.is_default()
        && private_key.parent() == Some(paths.dir())
        && let Some(name) = private_key.file_name()
    {
        return format!("~/.ssh/{}", name.to_string_lossy());
    }
    private_key.to_string_lossy().replace('\\', "/")
}

/// An in-memory copy of a config file.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    pub path: PathBuf,
    pub exists: bool,
    pub lines: Vec<String>,
    /// `"\r\n"` if the file used CRLF, else `"\n"`.
    pub newline: &'static str,
    /// Set when the file had an encoding ssh cannot read (BOM / UTF-16).
    pub encoding_issue: Option<&'static str>,
}

/// Result of [`ConfigFile::merge`].
#[derive(Debug, Clone)]
pub struct Merged {
    pub lines: Vec<String>,
    pub action: MergeAction,
}

impl ConfigFile {
    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => Ok(Self::from_bytes(path, &bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self {
                path: path.to_owned(),
                exists: false,
                lines: Vec::new(),
                newline: "\n",
                encoding_issue: None,
            }),
            Err(e) => Err(e),
        }
    }

    pub fn from_bytes(path: &Path, bytes: &[u8]) -> Self {
        let (text, encoding_issue) = decode(bytes);
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let mut lines: Vec<String> = text.split('\n').map(|l| l.trim_end_matches('\r').to_owned()).collect();
        if lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        Self {
            path: path.to_owned(),
            exists: true,
            lines,
            newline,
            encoding_issue,
        }
    }

    pub fn sections(&self) -> Vec<Section> {
        sections(&self.lines)
    }

    /// The `Host` section whose patterns include `alias` (case-insensitive).
    pub fn find_host(&self, alias: &str) -> Option<Section> {
        find_host(&self.sections(), alias)
    }

    /// The text of a section, for showing what would be replaced.
    pub fn section_text(&self, section: &Section) -> String {
        self.lines[section.start..=section.end].join("\n")
    }

    /// Adds `entry_lines` (from [`HostEntry::to_lines`]) for `alias`.
    pub fn merge(&self, alias: &str, entry_lines: &[String], replace: bool) -> Result<Merged, ConfigError> {
        let sections = self.sections();
        let lines = &self.lines;
        let mut out = Vec::with_capacity(lines.len() + entry_lines.len() + 2);

        if let Some(existing) = find_host(&sections, alias) {
            if existing.patterns.len() > 1 {
                return Err(ConfigError::AliasInMultiHost {
                    alias: alias.to_owned(),
                    patterns: existing.patterns.join(" "),
                });
            }
            if !replace {
                return Err(ConfigError::AliasExists(alias.to_owned()));
            }
            out.extend_from_slice(&lines[..existing.start]);
            out.extend_from_slice(entry_lines);
            out.extend_from_slice(&lines[existing.end + 1..]);
            return Ok(Merged {
                lines: out,
                action: MergeAction::Replaced,
            });
        }

        let wildcard = sections
            .iter()
            .find(|s| s.keyword == Keyword::Host && s.patterns.len() == 1 && s.patterns[0] == "*");
        if let Some(wildcard) = wildcard {
            // Comments directly above `Host *` belong to it.
            let mut at = wildcard.start;
            while at > 0 && lines[at - 1].trim_start().starts_with('#') {
                at -= 1;
            }
            out.extend_from_slice(&lines[..at]);
            if at > 0 && !lines[at - 1].trim().is_empty() {
                out.push(String::new());
            }
            out.extend_from_slice(entry_lines);
            out.push(String::new());
            out.extend_from_slice(&lines[at..]);
            return Ok(Merged {
                lines: out,
                action: MergeAction::InsertedBeforeWildcard,
            });
        }

        out.extend_from_slice(lines);
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.extend_from_slice(entry_lines);
        Ok(Merged {
            lines: out,
            action: MergeAction::Appended,
        })
    }

    /// Writes `lines` as UTF-8 without BOM, keeping the file's newline style.
    /// An existing file is copied to `config.bak-<timestamp>` first; the
    /// backup path is returned.
    pub fn write(&self, lines: &[String]) -> io::Result<Option<PathBuf>> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let backup = if self.exists {
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
            let mut name = self.path.clone().into_os_string();
            name.push(format!(".bak-{stamp}"));
            let backup = PathBuf::from(name);
            fs::copy(&self.path, &backup)?;
            Some(backup)
        } else {
            None
        };
        let mut text = lines.join(self.newline);
        text.push_str(self.newline);
        fs::write(&self.path, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(backup)
    }
}

fn decode(bytes: &[u8]) -> (String, Option<&'static str>) {
    let utf16 = |rest: &[u8], le: bool| -> String {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| {
                if le {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => (String::from_utf8_lossy(rest).into_owned(), Some("UTF-8 with BOM")),
        [0xFF, 0xFE, rest @ ..] => (utf16(rest, true), Some("UTF-16 LE")),
        [0xFE, 0xFF, rest @ ..] => (utf16(rest, false), Some("UTF-16 BE")),
        _ => (String::from_utf8_lossy(bytes).into_owned(), None),
    }
}

fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.is_empty() || t.starts_with('#')
}

/// Recognizes `Host a b`, `Host=a`, `match all` (keywords are case-insensitive).
fn parse_header(line: &str) -> Option<(Keyword, Vec<String>)> {
    let t = line.trim_start();
    let starts = |kw: &str| t.get(..kw.len()).is_some_and(|s| s.eq_ignore_ascii_case(kw));
    let (keyword, rest) = if starts("host") {
        (Keyword::Host, &t[4..])
    } else if starts("match") {
        (Keyword::Match, &t[5..])
    } else {
        return None;
    };
    // The keyword must be followed by whitespace and/or '=' — this rejects
    // `HostName`, `HostKeyAlias`, ...
    let trimmed = rest.trim_start();
    let args = match trimmed.strip_prefix('=') {
        Some(after) => after,
        None if trimmed.len() < rest.len() => trimmed,
        None => return None,
    };
    let patterns = args.replace('"', "").split_whitespace().map(str::to_owned).collect();
    Some((keyword, patterns))
}

pub fn sections(lines: &[String]) -> Vec<Section> {
    let headers: Vec<(usize, Keyword, Vec<String>)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| parse_header(l).map(|(k, p)| (i, k, p)))
        .collect();
    let mut out = Vec::with_capacity(headers.len());
    for (n, (start, keyword, patterns)) in headers.iter().enumerate() {
        let next = headers.get(n + 1).map_or(lines.len(), |h| h.0);
        let mut end = next - 1;
        while end > *start && is_blank_or_comment(&lines[end]) {
            end -= 1;
        }
        out.push(Section {
            keyword: *keyword,
            start: *start,
            end,
            patterns: patterns.clone(),
        });
    }
    out
}

fn find_host(sections: &[Section], alias: &str) -> Option<Section> {
    sections
        .iter()
        .find(|s| s.keyword == Keyword::Host && s.patterns.iter().any(|p| p.eq_ignore_ascii_case(alias)))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(text: &str) -> ConfigFile {
        ConfigFile::from_bytes(Path::new("config"), text.as_bytes())
    }

    fn entry(alias: &str) -> Vec<String> {
        HostEntry {
            alias: alias.into(),
            host_name: "10.0.0.5".into(),
            user: "ubuntu".into(),
            port: 22,
            identity_file: "~/.ssh/id_ed25519".into(),
        }
        .to_lines("2026-01-01")
    }

    #[test]
    fn entry_lines() {
        let lines = HostEntry {
            alias: "win".into(),
            host_name: "fe80::1".into(),
            user: "John Doe".into(),
            port: 2222,
            identity_file: "C:/keys/my key".into(),
        }
        .to_lines("2026-01-01");
        assert_eq!(
            lines,
            [
                "Host win",
                "    # added by sshelper 2026-01-01",
                "    HostName fe80::1",
                "    User \"John Doe\"",
                "    Port 2222",
                "    IdentityFile \"C:/keys/my key\"",
                "    IdentitiesOnly yes",
            ]
        );
    }

    #[test]
    fn header_parsing() {
        assert_eq!(
            parse_header("Host a b"),
            Some((Keyword::Host, vec!["a".into(), "b".into()]))
        );
        assert_eq!(parse_header("  host=a"), Some((Keyword::Host, vec!["a".into()])));
        assert_eq!(parse_header("Host = \"a\""), Some((Keyword::Host, vec!["a".into()])));
        assert_eq!(parse_header("Match all").map(|h| h.0), Some(Keyword::Match));
        assert_eq!(parse_header("    HostName x"), None);
        assert_eq!(parse_header("HostKeyAlias x"), None);
        assert_eq!(parse_header("# Host x"), None);
        assert_eq!(parse_header("主機 x"), None);
    }

    #[test]
    fn sections_exclude_trailing_comments() {
        let c = cfg("Host a\n  User x\n\n# about b\nHost b\n  User y\n");
        let s = c.sections();
        assert_eq!((s[0].start, s[0].end), (0, 1));
        assert_eq!((s[1].start, s[1].end), (4, 5));
    }

    #[test]
    fn appends_to_empty_and_plain_files() {
        let merged = cfg("").merge("jetson", &entry("jetson"), false).unwrap();
        assert_eq!(merged.action, MergeAction::Appended);
        assert_eq!(merged.lines[0], "Host jetson");

        let merged = cfg("Host a\n  User x\n")
            .merge("jetson", &entry("jetson"), false)
            .unwrap();
        assert_eq!(merged.lines[..3], ["Host a", "  User x", ""]);
        assert_eq!(merged.lines[3], "Host jetson");
    }

    #[test]
    fn inserts_before_wildcard_with_its_comment() {
        let c = cfg("Host a\n  User x\n\n# defaults\nHost *\n  User root\n");
        let merged = c.merge("jetson", &entry("jetson"), false).unwrap();
        assert_eq!(merged.action, MergeAction::InsertedBeforeWildcard);
        let text = merged.lines.join("\n");
        assert!(text.starts_with("Host a\n  User x\n\nHost jetson\n"), "{text}");
        assert!(
            text.ends_with("IdentitiesOnly yes\n\n# defaults\nHost *\n  User root"),
            "{text}"
        );
    }

    #[test]
    fn replaces_existing_alias_in_place() {
        let c = cfg("Host other\n  User o\n\nhost JETSON\n  HostName old\n  User old\n\n# next\nHost z\n  User z\n");
        assert!(matches!(
            c.merge("jetson", &entry("jetson"), false),
            Err(ConfigError::AliasExists(_))
        ));
        let merged = c.merge("jetson", &entry("jetson"), true).unwrap();
        assert_eq!(merged.action, MergeAction::Replaced);
        let text = merged.lines.join("\n");
        assert!(!text.contains("HostName old"));
        assert!(text.contains("Host other\n  User o\n\nHost jetson\n"));
        assert!(text.contains("IdentitiesOnly yes\n\n# next\nHost z"));
    }

    #[test]
    fn refuses_multi_alias_section() {
        let c = cfg("Host jetson nano\n  User x\n");
        assert!(matches!(
            c.merge("jetson", &entry("jetson"), true),
            Err(ConfigError::AliasInMultiHost { .. })
        ));
    }

    #[test]
    fn decodes_bom_and_utf16() {
        let c = ConfigFile::from_bytes(Path::new("c"), b"\xEF\xBB\xBFHost a\r\n  User x\r\n");
        assert_eq!(c.encoding_issue, Some("UTF-8 with BOM"));
        assert_eq!(c.newline, "\r\n");
        assert_eq!(c.lines, ["Host a", "  User x"]);

        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend("Host b\r\n".encode_utf16().flat_map(u16::to_le_bytes));
        let c = ConfigFile::from_bytes(Path::new("c"), &utf16);
        assert_eq!(c.encoding_issue, Some("UTF-16 LE"));
        assert_eq!(c.find_host("b").map(|s| s.start), Some(0));
    }

    #[test]
    fn write_keeps_newlines_and_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        fs::write(&path, b"\xEF\xBB\xBFHost a\r\n  User x\r\n").unwrap();

        let c = ConfigFile::load(&path).unwrap();
        let merged = c.merge("b", &entry("b"), false).unwrap();
        let backup = c.write(&merged.lines).unwrap().expect("backup");

        let written = fs::read(&path).unwrap();
        assert!(!written.starts_with(b"\xEF\xBB\xBF"));
        assert!(
            String::from_utf8(written)
                .unwrap()
                .starts_with("Host a\r\n  User x\r\n\r\nHost b\r\n")
        );
        assert!(fs::read(backup).unwrap().starts_with(b"\xEF\xBB\xBF"));
    }

    #[test]
    fn write_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let c = ConfigFile::load(&dir.path().join("sub").join("config")).unwrap();
        assert!(!c.exists);
        let merged = c.merge("a", &entry("a"), false).unwrap();
        assert_eq!(c.write(&merged.lines).unwrap(), None);
        assert!(fs::read_to_string(&c.path).unwrap().ends_with("IdentitiesOnly yes\n"));
    }

    #[test]
    fn identity_file_paths() {
        let custom = crate::SshPaths::custom("/tmp/x/.ssh");
        assert_eq!(
            identity_file_value(&custom, Path::new("/tmp/x/.ssh/id")),
            "/tmp/x/.ssh/id"
        );
        if let Some(real) = crate::SshPaths::for_current_user() {
            assert_eq!(
                identity_file_value(&real, &real.dir().join("id_ed25519")),
                "~/.ssh/id_ed25519"
            );
        }
    }
}
