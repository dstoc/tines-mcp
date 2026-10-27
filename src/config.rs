use std::{error::Error, fmt, fs, net::SocketAddr, path::PathBuf, str::FromStr};

use url::{Host, Url};

const DEFAULT_ALLOWED_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub allowed_hosts: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub tines: Option<TinesConfig>,
}

#[derive(Clone, Debug)]
pub struct TinesConfig {
    pub base_url: Url,
    pub web_url: Url,
    pub excluded_project_ids: Vec<String>,
    pub token_source: TokenSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenSource {
    Environment,
    File(PathBuf),
}

#[derive(Debug, Eq, PartialEq)]
pub struct ConfigError {
    variable: &'static str,
    reason: &'static str,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.variable, self.reason)
    }
}

impl Error for ConfigError {}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    pub fn from_lookup(mut get: impl FnMut(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let bind_addr = match get("MCP_BIND_ADDR") {
            Some(value) => SocketAddr::from_str(&value).map_err(|_| ConfigError {
                variable: "MCP_BIND_ADDR",
                reason: "must be a valid socket address",
            })?,
            None => SocketAddr::from(([127, 0, 0, 1], 8080)),
        };

        let allowed_hosts = match get("MCP_ALLOWED_HOSTS") {
            Some(value) => {
                let hosts = parse_csv(&value);
                if hosts.is_empty() {
                    return Err(ConfigError {
                        variable: "MCP_ALLOWED_HOSTS",
                        reason: "must contain at least one host",
                    });
                }
                for host in &hosts {
                    validate_host(host)?;
                }
                hosts
            }
            None => DEFAULT_ALLOWED_HOSTS.map(str::to_owned).to_vec(),
        };

        let allowed_origins = get("MCP_ALLOWED_ORIGINS")
            .map(|value| parse_csv(&value))
            .unwrap_or_default()
            .into_iter()
            .map(|origin| normalize_allowed_origin(&origin))
            .collect::<Result<Vec<_>, _>>()?;

        let allow_http = get("TINES_ALLOW_HTTP")
            .as_deref()
            .map(parse_bool)
            .transpose()?
            .unwrap_or(false);
        let base_url_raw = get("TINES_BASE_URL").filter(|value| !value.trim().is_empty());
        let token_env = get("TINES_API_TOKEN").filter(|value| !value.trim().is_empty());
        let token_file = get("TINES_API_TOKEN_FILE").filter(|value| !value.trim().is_empty());
        let web_url_raw = get("TINES_WEB_URL").filter(|value| !value.trim().is_empty());
        let excluded_project_ids = get("TINES_EXCLUDED_PROJECT_IDS")
            .map(|value| parse_csv(&value))
            .unwrap_or_default();

        for id in &excluded_project_ids {
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            {
                return Err(ConfigError {
                    variable: "TINES_EXCLUDED_PROJECT_IDS",
                    reason: "project IDs may contain only ASCII letters, digits, '_' and '-'",
                });
            }
        }

        let has_tines_settings = base_url_raw.is_some()
            || token_env.is_some()
            || token_file.is_some()
            || web_url_raw.is_some()
            || !excluded_project_ids.is_empty();

        let tines = if has_tines_settings {
            let base_url_raw = base_url_raw.ok_or(ConfigError {
                variable: "TINES_BASE_URL",
                reason: "is required when Tines settings are provided",
            })?;
            let token_source = match (token_env, token_file) {
                (Some(_), None) => TokenSource::Environment,
                (None, Some(path)) => {
                    let path = PathBuf::from(path);
                    let contents = fs::read(&path).map_err(|_| ConfigError {
                        variable: "TINES_API_TOKEN_FILE",
                        reason: "must name a readable, non-empty token file",
                    })?;
                    if contents.iter().all(u8::is_ascii_whitespace) {
                        return Err(ConfigError {
                            variable: "TINES_API_TOKEN_FILE",
                            reason: "must name a readable, non-empty token file",
                        });
                    }
                    TokenSource::File(path)
                }
                (Some(_), Some(_)) => {
                    return Err(ConfigError {
                        variable: "TINES_API_TOKEN_FILE",
                        reason: "set exactly one of TINES_API_TOKEN_FILE or TINES_API_TOKEN",
                    });
                }
                (None, None) => {
                    return Err(ConfigError {
                        variable: "TINES_API_TOKEN_FILE",
                        reason: "set exactly one of TINES_API_TOKEN_FILE or TINES_API_TOKEN",
                    });
                }
            };

            let base_url = validate_tines_url(&base_url_raw, allow_http, "TINES_BASE_URL")?;
            let web_url = match web_url_raw {
                Some(value) => validate_tines_url(&value, allow_http, "TINES_WEB_URL")?,
                None => base_url.clone(),
            };

            Some(TinesConfig {
                base_url,
                web_url,
                excluded_project_ids,
                token_source,
            })
        } else {
            None
        };

        Ok(Self {
            bind_addr,
            allowed_hosts,
            allowed_origins,
            tines,
        })
    }
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_bool(value: &str) -> Result<bool, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ConfigError {
            variable: "TINES_ALLOW_HTTP",
            reason: "must be 'true' or 'false'",
        }),
    }
}

fn validate_host(value: &str) -> Result<(), ConfigError> {
    let parsed = Url::parse(&format!("http://{value}")).map_err(|_| ConfigError {
        variable: "MCP_ALLOWED_HOSTS",
        reason: "entries must be host names or host:port authorities",
    })?;
    if parsed.host().is_none()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || value.contains(['/', '@', '*', ' ', '\t'])
    {
        return Err(ConfigError {
            variable: "MCP_ALLOWED_HOSTS",
            reason: "entries must be host names or host:port authorities",
        });
    }
    Ok(())
}

fn validate_origin(value: &str, variable: &'static str) -> Result<Url, ConfigError> {
    let raw_suffix = value
        .split_once("://")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.find(['/', '?', '#']).map(|index| &rest[index..]));
    let parsed = Url::parse(value).map_err(|_| ConfigError {
        variable,
        reason: "must be an exact HTTP or HTTPS origin",
    })?;
    if !matches!(parsed.scheme(), "http" | "https")
        || value.trim() != value
        || value.bytes().any(|byte| byte.is_ascii_whitespace())
        || parsed.host().is_none()
        || parsed.path() != "/"
        || raw_suffix.is_some_and(|suffix| suffix != "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(ConfigError {
            variable,
            reason: "must be an exact HTTP or HTTPS origin",
        });
    }
    Ok(parsed)
}

fn normalize_allowed_origin(value: &str) -> Result<String, ConfigError> {
    const VARIABLE: &str = "MCP_ALLOWED_ORIGINS";
    const REASON: &str = "must be an exact HTTP or HTTPS origin";

    let parsed = validate_origin(value, VARIABLE)?;
    let host = parsed.host_str().ok_or(ConfigError {
        variable: VARIABLE,
        reason: REASON,
    })?;
    let port = parsed.port_or_known_default().ok_or(ConfigError {
        variable: VARIABLE,
        reason: REASON,
    })?;

    // Avoid Url serialization here: it omits explicit default ports, which the
    // SDK interprets as an allowlist entry matching every port.
    Ok(format!("{}://{host}:{port}", parsed.scheme()))
}

fn validate_tines_url(
    value: &str,
    allow_http: bool,
    variable: &'static str,
) -> Result<Url, ConfigError> {
    let parsed = validate_origin(value, variable)?;
    let secure = parsed.scheme() == "https";
    let allowed_loopback_http = parsed.scheme() == "http" && allow_http && is_loopback(&parsed);
    if !secure && !allowed_loopback_http {
        return Err(ConfigError {
            variable,
            reason: "must use HTTPS; HTTP is allowed only for loopback when TINES_ALLOW_HTTP=true",
        });
    }
    Ok(parsed)
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(name)) => {
            name.eq_ignore_ascii_case("localhost") || name.ends_with(".localhost")
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs};

    use super::{Config, TokenSource};

    fn config(values: &[(&str, &str)]) -> Result<Config, super::ConfigError> {
        let values: HashMap<_, _> = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        Config::from_lookup(|name| values.get(name).cloned())
    }

    #[test]
    fn defaults_allow_local_start_without_tines_credentials() {
        let config = config(&[]).expect("default config should be valid");
        assert_eq!(config.bind_addr.to_string(), "127.0.0.1:8080");
        assert_eq!(config.allowed_hosts, ["localhost", "127.0.0.1", "[::1]"]);
        assert!(config.allowed_origins.is_empty());
        assert!(config.tines.is_none());
    }

    #[test]
    fn validates_tines_url_and_keeps_environment_secret_out_of_config() {
        let config = config(&[
            ("TINES_BASE_URL", "https://tines.example"),
            ("TINES_API_TOKEN", "not-a-real-token"),
            ("TINES_EXCLUDED_PROJECT_IDS", "prj_one, prj_two"),
        ])
        .expect("HTTPS origin and one token source should be valid");
        let tines = config
            .tines
            .as_ref()
            .expect("Tines config should be present");
        assert_eq!(tines.base_url.as_str(), "https://tines.example/");
        assert_eq!(tines.web_url, tines.base_url);
        assert_eq!(tines.excluded_project_ids, ["prj_one", "prj_two"]);
        assert_eq!(tines.token_source, TokenSource::Environment);
        assert!(!format!("{config:?}").contains("not-a-real-token"));
    }

    #[test]
    fn accepts_loopback_http_only_when_explicitly_enabled() {
        assert!(config(&[("TINES_BASE_URL", "http://localhost:3000")]).is_err());
        let config = config(&[
            ("TINES_BASE_URL", "http://localhost:3000"),
            ("TINES_API_TOKEN", "dummy"),
            ("TINES_ALLOW_HTTP", "true"),
        ])
        .expect("explicit loopback HTTP should be valid");
        assert_eq!(
            config
                .tines
                .expect("Tines config should be present")
                .base_url
                .as_str(),
            "http://localhost:3000/"
        );
    }

    #[test]
    fn rejects_ambiguous_or_incomplete_token_configuration() {
        assert!(config(&[("TINES_BASE_URL", "https://tines.example")]).is_err());
        assert!(
            config(&[
                ("TINES_BASE_URL", "https://tines.example"),
                ("TINES_API_TOKEN", "dummy"),
                ("TINES_API_TOKEN_FILE", "/tmp/unused"),
            ])
            .is_err()
        );
    }

    #[test]
    fn validates_token_file_without_exposing_contents() {
        let path = std::env::temp_dir().join(format!("tines-mcp-token-{}", std::process::id()));
        fs::write(&path, "dummy-token\n").expect("token fixture should be writable");
        let path_string = path.to_string_lossy().into_owned();
        let config = config(&[
            ("TINES_BASE_URL", "https://tines.example"),
            ("TINES_API_TOKEN_FILE", path_string.as_str()),
        ])
        .expect("readable token file should be valid");
        let _ = fs::remove_file(&path);
        assert_eq!(
            config
                .tines
                .expect("Tines config should be present")
                .token_source,
            TokenSource::File(path)
        );
    }

    #[test]
    fn rejects_untrusted_origin_and_invalid_host_entries() {
        assert!(config(&[("MCP_ALLOWED_ORIGINS", "https://client.example/path")]).is_err());
        assert!(config(&[("MCP_ALLOWED_ORIGINS", "https://client.example/path/..")]).is_err());
        assert!(config(&[("MCP_ALLOWED_HOSTS", "* ")]).is_err());
    }

    #[test]
    fn normalizes_allowed_origins_to_explicit_effective_ports() {
        let config = config(&[(
            "MCP_ALLOWED_ORIGINS",
            "https://client.example, http://localhost, https://[2001:db8::1]:8443",
        )])
        .expect("valid HTTP origins should be normalized");

        assert_eq!(
            config.allowed_origins,
            [
                "https://client.example:443",
                "http://localhost:80",
                "https://[2001:db8::1]:8443"
            ]
        );
    }
}
