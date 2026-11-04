//! Configuration: a `KEY=value` file plus environment overrides.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

pub const DEFAULT_PREFIX: &str = "/client";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub server_url: String,
    pub token: String,
    /// Normalised: empty, or starts with `/` and has no trailing `/`.
    pub prefix: String,
}

impl Config {
    /// Base URL every API path is appended to, e.g. `https://host/client`.
    pub fn base_url(&self) -> String {
        format!("{}{}", self.server_url.trim_end_matches('/'), self.prefix)
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Missing(PathBuf),
    Unreadable(PathBuf, String),
    Incomplete(Vec<&'static str>),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Missing(p) => write!(f, "No config file found at {}.", p.display())?,
            ConfigError::Unreadable(p, e) => write!(f, "Could not read {}: {e}", p.display())?,
            ConfigError::Incomplete(keys) => {
                write!(f, "Config is incomplete: {} not set.", keys.join(" and "))?
            }
        }
        write!(f, "\n\n{SETUP_HELP}")
    }
}

impl std::error::Error for ConfigError {}

pub const SETUP_HELP: &str = "\
Create the file (see config.example in the repo) with:

    SERVER_URL=https://your-ideas-host
    CLIENT_TOKEN=<token the server accepts for the client API>
    API_PREFIX=/client            # optional, default /client

then protect it:  chmod 600 <config file>

Environment variables SERVER_URL, CLIENT_TOKEN and API_PREFIX override the file.";

/// Where the config file lives: `$XDG_CONFIG_HOME/ideas-tui/config`, else
/// `~/.config/ideas-tui/config`.
pub fn config_path(xdg: Option<&str>, home: Option<&str>) -> PathBuf {
    match xdg.filter(|s| !s.is_empty()) {
        Some(x) => PathBuf::from(x).join("ideas-tui/config"),
        None => PathBuf::from(home.unwrap_or(".")).join(".config/ideas-tui/config"),
    }
}

/// Parses dotenv-style text: `KEY=value`, `#` comments, optional `export `,
/// optional single or double quotes around the value.
pub fn parse_dotenv(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else { continue };
        let mut v = v.trim();
        if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) {
            v = &v[1..v.len() - 1];
        } else if let Some((before, _)) = v.split_once(" #") {
            v = before.trim_end();
        }
        out.insert(k.trim().to_string(), v.to_string());
    }
    out
}

pub fn normalize_prefix(p: &str) -> String {
    let p = p.trim().trim_matches('/');
    if p.is_empty() { String::new() } else { format!("/{p}") }
}

/// Merges file values with environment overrides and validates.
pub fn resolve(file: &HashMap<String, String>, env: &dyn Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
    let get = |k: &str| {
        env(k)
            .or_else(|| file.get(k).cloned())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let server_url = get("SERVER_URL");
    let token = get("CLIENT_TOKEN");
    let mut missing = Vec::new();
    if server_url.is_none() {
        missing.push("SERVER_URL");
    }
    if token.is_none() {
        missing.push("CLIENT_TOKEN");
    }
    if !missing.is_empty() {
        return Err(ConfigError::Incomplete(missing));
    }
    // An explicitly empty API_PREFIX means "no prefix" (handy against a raw server).
    let prefix = match env("API_PREFIX").or_else(|| file.get("API_PREFIX").cloned()) {
        Some(p) => normalize_prefix(&p),
        None => DEFAULT_PREFIX.to_string(),
    };
    Ok(Config { server_url: server_url.unwrap(), token: token.unwrap(), prefix })
}

/// True if the file is accessible by group or others.
#[cfg(unix)]
pub fn too_permissive(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).map(|m| m.permissions().mode() & 0o077 != 0).unwrap_or(false)
}
#[cfg(not(unix))]
pub fn too_permissive(_path: &Path) -> bool {
    false
}

/// Loads the config from disk and the process environment. The file may be
/// absent as long as the environment supplies everything. Returns warnings
/// to show the user.
pub fn load() -> Result<(Config, Vec<String>), ConfigError> {
    let path = config_path(
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    );
    let mut warnings = Vec::new();
    let file = match std::fs::read_to_string(&path) {
        Ok(text) => {
            if too_permissive(&path) {
                warnings.push(format!("{} is readable by others; run: chmod 600 {}", path.display(), path.display()));
            }
            parse_dotenv(&text)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(e) => return Err(ConfigError::Unreadable(path, e.to_string())),
    };
    match resolve(&file, &|k| std::env::var(k).ok()) {
        Ok(c) => Ok((c, warnings)),
        Err(ConfigError::Incomplete(_)) if file.is_empty() && !path.exists() => Err(ConfigError::Missing(path)),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn parses_dotenv() {
        let m = parse_dotenv("# c\n\nSERVER_URL=https://x.test\nexport CLIENT_TOKEN=\"abc def\"\nAPI_PREFIX='/c' \nBAD\nA=b # note\n");
        assert_eq!(m["SERVER_URL"], "https://x.test");
        assert_eq!(m["CLIENT_TOKEN"], "abc def");
        assert_eq!(m["API_PREFIX"], "/c");
        assert_eq!(m["A"], "b");
        assert!(!m.contains_key("BAD"));
    }

    #[test]
    fn default_prefix_and_base_url() {
        let f = parse_dotenv("SERVER_URL=https://x.test/\nCLIENT_TOKEN=t");
        let c = resolve(&f, &no_env).unwrap();
        assert_eq!(c.prefix, "/client");
        assert_eq!(c.base_url(), "https://x.test/client");
    }

    #[test]
    fn env_overrides_file() {
        let f = parse_dotenv("SERVER_URL=https://a.test\nCLIENT_TOKEN=t\nAPI_PREFIX=/old");
        let env = |k: &str| match k {
            "SERVER_URL" => Some("http://localhost:1".to_string()),
            "API_PREFIX" => Some("api/v2/".to_string()),
            _ => None,
        };
        let c = resolve(&f, &env).unwrap();
        assert_eq!(c.server_url, "http://localhost:1");
        assert_eq!(c.token, "t");
        assert_eq!(c.prefix, "/api/v2");
    }

    #[test]
    fn empty_prefix_means_none() {
        let f = parse_dotenv("SERVER_URL=http://h\nCLIENT_TOKEN=t\nAPI_PREFIX=");
        assert_eq!(resolve(&f, &no_env).unwrap().base_url(), "http://h");
    }

    #[test]
    fn reports_missing_keys() {
        let f = parse_dotenv("SERVER_URL=http://h");
        match resolve(&f, &no_env) {
            Err(ConfigError::Incomplete(k)) => assert_eq!(k, vec!["CLIENT_TOKEN"]),
            other => panic!("{other:?}"),
        }
        let msg = resolve(&HashMap::new(), &no_env).unwrap_err().to_string();
        assert!(msg.contains("SERVER_URL and CLIENT_TOKEN"));
        assert!(msg.contains("chmod 600"));
    }

    #[test]
    fn config_path_honours_xdg() {
        assert_eq!(config_path(Some("/x"), Some("/h")), PathBuf::from("/x/ideas-tui/config"));
        assert_eq!(config_path(None, Some("/h")), PathBuf::from("/h/.config/ideas-tui/config"));
        assert_eq!(config_path(Some(""), Some("/h")), PathBuf::from("/h/.config/ideas-tui/config"));
    }

    #[cfg(unix)]
    #[test]
    fn detects_loose_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ideas-tui-perm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("config");
        std::fs::write(&f, "x").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(too_permissive(&f));
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!too_permissive(&f));
        std::fs::remove_dir_all(&dir).ok();
    }
}
