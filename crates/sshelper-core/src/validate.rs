//! Validation of user input shared by the CLI and the GUI. Errors are
//! user-facing messages.

/// A parsed "host" field. Users often paste `user@host`, so the user part is
/// split off and offered as the default account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInput {
    pub host: String,
    pub user: Option<String>,
}

pub fn host(input: &str) -> Result<HostInput, String> {
    let input = input.trim();
    let (user, host) = match input.rsplit_once('@') {
        Some((user, host)) => (Some(user.to_owned()).filter(|u| !u.is_empty()), host),
        None => (None, input),
    };
    // Accept "[::1]" as well as "::1".
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if host.is_empty() {
        return Err("請輸入主機 IP 或網域名稱".into());
    }
    let valid = !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '%'));
    if !valid {
        return Err("主機格式不正確，例如 192.168.1.10、jetson.local 或 fe80::1".into());
    }
    if let Some(user) = &user {
        self::user(user)?;
    }
    Ok(HostInput {
        host: host.to_owned(),
        user,
    })
}

pub fn user(user: &str) -> Result<(), String> {
    if user.trim().is_empty() {
        return Err("請輸入帳號".into());
    }
    if user != user.trim() || user.contains([':', '"', '\n', '\r']) {
        return Err("帳號不可包含冒號、雙引號，也不可有前後空白".into());
    }
    Ok(())
}

pub fn port(input: &str) -> Result<u16, String> {
    match input.trim().parse::<u16>() {
        Ok(p) if p > 0 => Ok(p),
        _ => Err("連接埠需為 1 ~ 65535".into()),
    }
}

/// Host aliases end up as `Host <alias>` lines, so wildcards, spaces and
/// leading dashes are rejected.
pub fn alias(alias: &str) -> Result<(), String> {
    let mut chars = alias.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err("別名只能使用英數字與 . _ -，且需以英數字開頭，例如 jetson、dev-server".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_accepts_plain_and_user_at_host() {
        assert_eq!(
            host("192.168.1.10").unwrap(),
            HostInput {
                host: "192.168.1.10".into(),
                user: None
            }
        );
        assert_eq!(
            host(" ubuntu@jetson.local ").unwrap(),
            HostInput {
                host: "jetson.local".into(),
                user: Some("ubuntu".into())
            }
        );
        assert_eq!(host("[fe80::1%eth0]").unwrap().host, "fe80::1%eth0");
    }

    #[test]
    fn host_rejects_garbage() {
        assert!(host("").is_err());
        assert!(host("bad host").is_err());
        assert!(host("-oProxyCommand=x").is_err());
        assert!(host("a/b").is_err());
    }

    #[test]
    fn port_range() {
        assert_eq!(port("22"), Ok(22));
        assert!(port("0").is_err());
        assert!(port("65536").is_err());
        assert!(port("abc").is_err());
    }

    #[test]
    fn alias_rules() {
        assert!(alias("jetson").is_ok());
        assert!(alias("dev-server.1").is_ok());
        assert!(alias("*").is_err());
        assert!(alias("-x").is_err());
        assert!(alias("a b").is_err());
        assert!(alias("").is_err());
    }

    #[test]
    fn user_rules() {
        assert!(user("ubuntu").is_ok());
        assert!(user(r"DOMAIN\john").is_ok());
        assert!(user("John Doe").is_ok());
        assert!(user("a:b").is_err());
        assert!(user(" x").is_err());
    }
}
