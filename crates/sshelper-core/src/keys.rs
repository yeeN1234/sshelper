//! Local key pairs: discovery in `~/.ssh`, public key parsing and private key
//! permissions.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Key type prefixes of OpenSSH public keys (`ssh-ed25519`, `ecdsa-sha2-*`,
/// `sk-ssh-ed25519@openssh.com`, ...).
const KEY_PREFIXES: &[&str] = &["ssh-", "ecdsa-", "sk-"];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublicKeyError {
    #[error("公鑰檔案是空的")]
    Empty,
    #[error("不是 OpenSSH 公鑰格式（開頭應為 ssh-、ecdsa- 或 sk-）")]
    UnknownFormat,
    #[error("公鑰缺少金鑰內容")]
    MissingData,
    #[error("無法讀取公鑰：{0}")]
    Unreadable(String),
}

/// The parsed first line of a `.pub` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKeyInfo {
    pub algorithm: String,
    pub data: String,
    pub comment: String,
}

impl PublicKeyInfo {
    /// Parses the first non-empty line. A UTF-8 BOM and CRLF line endings, as
    /// left behind by Windows editors, are tolerated.
    pub fn parse(bytes: &[u8]) -> Result<Self, PublicKeyError> {
        let text = String::from_utf8_lossy(bytes);
        let line = text
            .trim_start_matches('\u{feff}')
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .ok_or(PublicKeyError::Empty)?;
        let mut parts = line.split_whitespace();
        let algorithm = parts.next().ok_or(PublicKeyError::Empty)?;
        if !KEY_PREFIXES.iter().any(|p| algorithm.starts_with(p)) {
            return Err(PublicKeyError::UnknownFormat);
        }
        let data = parts.next().ok_or(PublicKeyError::MissingData)?;
        Ok(Self {
            algorithm: algorithm.to_owned(),
            data: data.to_owned(),
            comment: parts.collect::<Vec<_>>().join(" "),
        })
    }
}

/// A `.pub` file and the private key next to it (same name without `.pub`).
#[derive(Debug, Clone)]
pub struct KeyPair {
    pub public_path: PathBuf,
    pub private_path: PathBuf,
    pub has_private: bool,
    pub public: Result<PublicKeyInfo, PublicKeyError>,
}

impl KeyPair {
    /// Builds the pair from a `.pub` path.
    pub fn from_public_path(public_path: impl Into<PathBuf>) -> Self {
        let public_path = public_path.into();
        let private_path = public_path.with_extension("");
        let public = fs::read(&public_path)
            .map_err(|e| PublicKeyError::Unreadable(e.to_string()))
            .and_then(|bytes| PublicKeyInfo::parse(&bytes));
        Self {
            has_private: private_path.is_file(),
            public_path,
            private_path,
            public,
        }
    }

    pub fn file_name(&self) -> String {
        file_name(&self.public_path)
    }

    pub fn private_file_name(&self) -> String {
        file_name(&self.private_path)
    }

    /// Usable for deployment: parsable public key and an existing private key.
    pub fn problem(&self) -> Option<String> {
        match &self.public {
            Err(e) => Some(e.to_string()),
            Ok(_) if !self.has_private => Some("找不到對應的私鑰".into()),
            Ok(_) => None,
        }
    }

    /// One-line description such as `id_ed25519.pub  ssh-ed25519  me@pc`.
    pub fn describe(&self) -> String {
        let mut text = self.file_name();
        if let Ok(info) = &self.public {
            text.push_str("  ");
            text.push_str(&info.algorithm);
            if !info.comment.is_empty() {
                text.push_str("  ");
                text.push_str(&info.comment);
            }
        }
        text
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// All `*.pub` files in `ssh_dir`, sorted by name. A missing directory yields
/// an empty list.
pub fn discover(ssh_dir: &Path) -> io::Result<Vec<KeyPair>> {
    let entries = match fs::read_dir(ssh_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut pairs = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let is_pub = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("pub"));
        if is_pub && path.is_file() {
            pairs.push(KeyPair::from_public_path(path));
        }
    }
    pairs.sort_by_key(|p| p.file_name().to_lowercase());
    Ok(pairs)
}

/// Resolves a key given on the command line: a path to the public or the
/// private key, or a name inside `ssh_dir` (`id_ed25519` / `id_ed25519.pub`).
pub fn resolve(ssh_dir: &Path, spec: &str) -> Option<KeyPair> {
    let candidates = [PathBuf::from(spec), ssh_dir.join(spec)];
    for path in candidates {
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pub")) && path.is_file() {
            return Some(KeyPair::from_public_path(path));
        }
        let mut with_pub = path.into_os_string();
        with_pub.push(".pub");
        let with_pub = PathBuf::from(with_pub);
        if with_pub.is_file() {
            return Some(KeyPair::from_public_path(with_pub));
        }
    }
    None
}

/// `Some(true)` if the private key needs a passphrase, `None` if unknown.
pub fn is_encrypted(private_path: &Path) -> Option<bool> {
    match russh::keys::load_secret_key(private_path, None) {
        Ok(_) => Some(false),
        Err(russh::keys::Error::KeyIsEncrypted) => Some(true),
        Err(_) => None,
    }
}

/// Restricts the private key to the current user so OpenSSH does not refuse
/// it with `WARNING: UNPROTECTED PRIVATE KEY FILE!`.
///
/// * Unix: `chmod 600`.
/// * Windows: `icacls /reset`, then `/inheritance:r /grant:r *<SID>:(R,W)` —
///   inheritance off and only the current user (by SID, so localized or
///   domain account names do not matter) may read and write.
pub fn protect_private_key(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }
    #[cfg(windows)]
    {
        let sid = windows_acl::current_user_sid()?;
        let path = path.to_string_lossy();
        windows_acl::icacls(&[&path, "/reset"])?;
        windows_acl::icacls(&[&path, "/inheritance:r", "/grant:r", &format!("*{sid}:(R,W)")])
    }
}

/// `chmod 700 ~/.ssh` on Unix; Windows relies on the profile ACL.
pub fn protect_ssh_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        let _ = dir;
        Ok(())
    }
}

#[cfg(windows)]
mod windows_acl {
    use crate::process::{command, tail};
    use std::io;

    /// SID of the current user, from `whoami /user /fo csv /nh`
    /// (`"pc\name","S-1-5-21-..."`).
    pub fn current_user_sid() -> io::Result<String> {
        let out = command("whoami").args(["/user", "/fo", "csv", "/nh"]).output()?;
        let text = String::from_utf8_lossy(&out.stdout);
        text.rsplit(',')
            .next()
            .map(|s| s.trim().trim_matches('"').to_owned())
            .filter(|s| s.starts_with("S-1-"))
            .ok_or_else(|| io::Error::other("無法取得目前使用者的 SID（whoami /user）"))
    }

    pub fn icacls(args: &[&str]) -> io::Result<()> {
        let out = command("icacls").args(args).output()?;
        if out.status.success() {
            Ok(())
        } else {
            let mut detail = tail(&out.stderr, 2);
            if detail.is_empty() {
                detail = tail(&out.stdout, 2);
            }
            Err(io::Error::other(format!(
                "icacls {} 失敗：{detail}",
                args[1..].join(" ")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_key() {
        let info = PublicKeyInfo::parse(b"ssh-ed25519 AAAAC3Nz me@pc\n").unwrap();
        assert_eq!(info.algorithm, "ssh-ed25519");
        assert_eq!(info.data, "AAAAC3Nz");
        assert_eq!(info.comment, "me@pc");
    }

    #[test]
    fn parses_bom_crlf_and_leading_blank_lines() {
        let info = PublicKeyInfo::parse(b"\xEF\xBB\xBF\r\n\r\nssh-rsa AAAAB3 two words\r\n").unwrap();
        assert_eq!(info.algorithm, "ssh-rsa");
        assert_eq!(info.comment, "two words");
    }

    #[test]
    fn rejects_non_keys() {
        assert_eq!(PublicKeyInfo::parse(b""), Err(PublicKeyError::Empty));
        assert_eq!(
            PublicKeyInfo::parse(b"-----BEGIN OPENSSH PRIVATE KEY-----"),
            Err(PublicKeyError::UnknownFormat)
        );
        assert_eq!(PublicKeyInfo::parse(b"ssh-ed25519"), Err(PublicKeyError::MissingData));
    }

    #[test]
    fn discovers_pairs_sorted() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("work.pub"), "ssh-rsa AAAA work").unwrap();
        fs::write(dir.path().join("work"), "private").unwrap();
        fs::write(dir.path().join("id_ed25519.pub"), "ssh-ed25519 AAAA me").unwrap();
        fs::write(dir.path().join("known_hosts"), "").unwrap();
        fs::write(dir.path().join("notes.pubx"), "").unwrap();

        let pairs = discover(dir.path()).unwrap();
        let names: Vec<_> = pairs.iter().map(KeyPair::file_name).collect();
        assert_eq!(names, ["id_ed25519.pub", "work.pub"]);
        assert!(!pairs[0].has_private);
        assert!(pairs[0].problem().is_some());
        assert!(pairs[1].has_private);
        assert_eq!(pairs[1].problem(), None);
        assert_eq!(pairs[1].private_file_name(), "work");
    }

    #[test]
    fn missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(discover(&dir.path().join("nope")).unwrap().is_empty());
    }

    #[test]
    fn resolves_by_name_or_path() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("id_ed25519.pub"), "ssh-ed25519 AAAA me").unwrap();
        fs::write(dir.path().join("id_ed25519"), "private").unwrap();
        for spec in ["id_ed25519", "id_ed25519.pub"] {
            assert_eq!(resolve(dir.path(), spec).unwrap().file_name(), "id_ed25519.pub");
        }
        let full = dir.path().join("id_ed25519");
        assert!(resolve(Path::new("/elsewhere"), full.to_str().unwrap()).is_some());
        assert!(resolve(dir.path(), "missing").is_none());
    }

    #[test]
    fn protects_private_key() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("id_test");
        fs::write(&key, "private").unwrap();
        protect_private_key(&key).unwrap();
        assert_eq!(fs::read_to_string(&key).unwrap(), "private");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&key).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}
