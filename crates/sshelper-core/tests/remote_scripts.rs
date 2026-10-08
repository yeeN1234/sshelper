//! Runs the embedded installer scripts locally against a fake home directory.
//! Each test is skipped when its interpreter is not available.

use std::fs;
use std::path::Path;
use std::process::Command;

use sshelper_core::deploy::{INSTALL_PS1, INSTALL_SH};

const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBlah me@pc";
const OTHER: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ other@pc";

fn has(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Copies the script + a .pub with the given bytes into `home`, runs it and
/// returns (success, combined output).
fn run_sh(home: &Path, pub_bytes: &[u8]) -> (bool, String) {
    fs::write(home.join(".t.pub"), pub_bytes).unwrap();
    fs::write(home.join(".t.sh"), INSTALL_SH.replace('\r', "")).unwrap();
    let out = Command::new("sh")
        .current_dir(home)
        .env("HOME", home)
        .args([".t.sh", ".t.pub"])
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn sh_installer_appends_once_and_cleans_up() {
    if !has("sh", &["-c", "exit 0"]) {
        eprintln!("skipped: no sh");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let auth = home.join(".ssh").join("authorized_keys");

    // Existing file without a trailing newline: the key must not be glued on.
    fs::create_dir_all(home.join(".ssh")).unwrap();
    fs::write(&auth, OTHER).unwrap();

    // BOM + CRLF, as a Windows editor would save it.
    let (ok, out) = run_sh(home, format!("\u{feff}{KEY}\r\n").as_bytes());
    assert!(ok, "{out}");
    assert_eq!(fs::read_to_string(&auth).unwrap(), format!("{OTHER}\n{KEY}\n"));
    assert!(
        !home.join(".t.pub").exists() && !home.join(".t.sh").exists(),
        "temp files removed"
    );

    // Second run: already present, nothing appended.
    let (ok, out) = run_sh(home, KEY.as_bytes());
    assert!(ok, "{out}");
    assert!(out.contains("already present"), "{out}");
    assert_eq!(fs::read_to_string(&auth).unwrap(), format!("{OTHER}\n{KEY}\n"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&home.join(".ssh")), 0o700);
        assert_eq!(mode(&auth), 0o600);
    }
}

#[test]
fn sh_installer_rejects_non_keys() {
    if !has("sh", &["-c", "exit 0"]) {
        eprintln!("skipped: no sh");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let (ok, out) = run_sh(home.path(), b"-----BEGIN OPENSSH PRIVATE KEY-----\n");
    assert!(!ok);
    assert!(out.contains("does not look like"), "{out}");
    assert!(!home.path().join(".t.pub").exists());
}

#[cfg(windows)]
fn is_elevated() -> bool {
    has(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "if (([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole('Administrators')) { exit 0 } else { exit 1 }",
        ],
    )
}

#[cfg(windows)]
#[test]
fn ps1_installer_writes_utf8_lf_and_fixes_utf16() {
    if is_elevated() {
        // Elevated runs would also touch %ProgramData%\ssh\administrators_authorized_keys.
        eprintln!("skipped: elevated session");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let auth = home.join(".ssh").join("authorized_keys");

    // A file broken by `echo key >> authorized_keys` in Windows PowerShell 5.1.
    fs::create_dir_all(home.join(".ssh")).unwrap();
    let mut utf16 = vec![0xFF, 0xFE];
    utf16.extend(format!("{OTHER}\r\n").encode_utf16().flat_map(u16::to_le_bytes));
    fs::write(&auth, utf16).unwrap();

    let run = |pub_bytes: &[u8]| {
        fs::write(home.join(".t.pub"), pub_bytes).unwrap();
        fs::write(home.join(".t.ps1"), INSTALL_PS1).unwrap();
        let out = Command::new("powershell")
            .env("USERPROFILE", home)
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(home.join(".t.ps1"))
            .arg(".t.pub")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text)
    };

    let (ok, out) = run(format!("\u{feff}{KEY}\r\n").as_bytes());
    assert!(ok, "{out}");
    assert_eq!(fs::read(&auth).unwrap(), format!("{OTHER}\n{KEY}\n").into_bytes());
    assert!(
        !home.join(".t.pub").exists() && !home.join(".t.ps1").exists(),
        "temp files removed"
    );

    let (ok, out) = run(KEY.as_bytes());
    assert!(ok, "{out}");
    assert!(out.contains("already present"), "{out}");
    assert_eq!(fs::read(&auth).unwrap(), format!("{OTHER}\n{KEY}\n").into_bytes());

    let (ok, out) = run(b"not a key");
    assert!(!ok, "{out}");
}
