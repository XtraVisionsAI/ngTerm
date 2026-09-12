//! Import of a *declared subset* of OpenSSH client configuration into the
//! server list (UX-05).
//!
//! Supported per `Host` block: a single literal host alias, `HostName`,
//! `User`, `Port`, `IdentityFile` (matched by file name to a stored key; an
//! unmatched file is reported, not guessed). Everything else is either
//! ignored silently when harmless (`ServerAliveInterval`, `ForwardAgent`,
//! …) or makes the block *skipped with a reason* when it changes how the
//! connection is made and this platform does not model it (`ProxyJump`,
//! `ProxyCommand`, `Match`, `Include`, wildcard or multi-pattern `Host`).
//! The result is a preview the user confirms; nothing is created on parse.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParsedHost {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    /// Basename of `IdentityFile`, when given.
    pub identity_file: Option<String>,
    /// Directives seen in the block that the import ignores (informational).
    pub ignored: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkippedHost {
    pub pattern: String,
    pub reason: String,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ParseResult {
    pub hosts: Vec<ParsedHost>,
    pub skipped: Vec<SkippedHost>,
}

/// Directives whose presence means the connection is not what a plain
/// host/port/user entry would make; the block is skipped, not approximated.
const UNSUPPORTED: &[&str] = &[
    "proxyjump",
    "proxycommand",
    "localforward",
    "remoteforward",
    "dynamicforward",
    "certificatefile",
    "controlmaster",
    "controlpath",
    "remotecommand",
];

struct Block {
    patterns: Vec<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    identity: Option<String>,
    unsupported: Vec<String>,
    ignored: Vec<String>,
    bad_port: Option<String>,
}

impl Block {
    fn new(patterns: Vec<String>) -> Self {
        Self {
            patterns,
            host: None,
            port: None,
            user: None,
            identity: None,
            unsupported: Vec::new(),
            ignored: Vec::new(),
            bad_port: None,
        }
    }
}

fn split_directive(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // `Key value`, `Key=value`, `Key = value`
    let (k, v) = match line.find(|c: char| c == '=' || c.is_whitespace()) {
        Some(i) => (
            &line[..i],
            line[i..].trim_start_matches(|c: char| c == '=' || c.is_whitespace()),
        ),
        None => (line, ""),
    };
    Some((
        k.to_ascii_lowercase(),
        v.trim().trim_matches('"').to_string(),
    ))
}

fn is_literal_pattern(p: &str) -> bool {
    !p.is_empty() && !p.contains(['*', '?']) && !p.starts_with('!')
}

pub fn parse(text: &str) -> ParseResult {
    let mut result = ParseResult::default();
    let mut blocks: Vec<Block> = Vec::new();
    let mut in_match = false;
    let mut match_patterns: Vec<String> = Vec::new();

    for raw in text.lines() {
        let Some((key, value)) = split_directive(raw) else {
            continue;
        };
        match key.as_str() {
            "host" => {
                in_match = false;
                let patterns: Vec<String> = value.split_whitespace().map(str::to_string).collect();
                blocks.push(Block::new(patterns));
            }
            "match" => {
                in_match = true;
                match_patterns.push(format!("Match {}", value));
            }
            "include" => result.skipped.push(SkippedHost {
                pattern: format!("Include {}", value),
                reason: "Include 不会被展开；请把被包含文件的内容一并粘贴".into(),
            }),
            _ if in_match => {}
            _ => {
                let Some(b) = blocks.last_mut() else {
                    // Global options before any Host: not applied.
                    continue;
                };
                match key.as_str() {
                    "hostname" => b.host = Some(value),
                    "user" => b.user = Some(value),
                    "port" => match value.parse::<u16>() {
                        Ok(p) if p > 0 => b.port = Some(p),
                        _ => b.bad_port = Some(value),
                    },
                    "identityfile" => {
                        b.identity = Some(
                            value
                                .rsplit(['/', '\\'])
                                .next()
                                .unwrap_or(&value)
                                .to_string(),
                        )
                    }
                    k if UNSUPPORTED.contains(&k) => b.unsupported.push(k.to_string()),
                    k => b.ignored.push(k.to_string()),
                }
            }
        }
    }
    for m in match_patterns {
        result.skipped.push(SkippedHost {
            pattern: m,
            reason: "Match 块不受支持（条件式配置无法映射为固定服务器）".into(),
        });
    }

    for b in blocks {
        let joined = b.patterns.join(" ");
        if b.patterns.len() != 1 {
            result.skipped.push(SkippedHost {
                pattern: joined,
                reason: "多模式 Host 行不受支持；每个服务器需要单独的 Host 别名".into(),
            });
            continue;
        }
        let alias = b.patterns[0].clone();
        if !is_literal_pattern(&alias) {
            result.skipped.push(SkippedHost {
                pattern: alias,
                reason: "通配符 / 否定模式不是具体服务器".into(),
            });
            continue;
        }
        if !b.unsupported.is_empty() {
            result.skipped.push(SkippedHost {
                pattern: alias,
                reason: format!(
                    "使用了本平台不建模的指令：{}（跳板链 / 端口转发 / 证书暂不导入）",
                    b.unsupported.join(", ")
                ),
            });
            continue;
        }
        if let Some(p) = b.bad_port {
            result.skipped.push(SkippedHost {
                pattern: alias,
                reason: format!("Port 无效：{}", p),
            });
            continue;
        }
        result.hosts.push(ParsedHost {
            host: b.host.unwrap_or_else(|| alias.clone()),
            alias,
            port: b.port.unwrap_or(22),
            username: b.user,
            identity_file: b.identity,
            ignored: b.ignored,
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_supported_subset() {
        let r = parse(
            "# comment\nHost web-1\n  HostName 10.0.0.1\n  User deploy\n  Port 2222\n  IdentityFile ~/.ssh/id_deploy\n  ServerAliveInterval 30\n\nHost db\n  HostName=db.internal\n",
        );
        assert_eq!(r.skipped, vec![]);
        assert_eq!(r.hosts.len(), 2);
        let w = &r.hosts[0];
        assert_eq!(
            (w.alias.as_str(), w.host.as_str(), w.port),
            ("web-1", "10.0.0.1", 2222)
        );
        assert_eq!(w.username.as_deref(), Some("deploy"));
        assert_eq!(w.identity_file.as_deref(), Some("id_deploy"));
        assert_eq!(w.ignored, vec!["serveraliveinterval"]);
        let d = &r.hosts[1];
        assert_eq!(
            (d.host.as_str(), d.port, d.username.is_none()),
            ("db.internal", 22, true)
        );
    }

    #[test]
    fn unsupported_shapes_are_skipped_with_a_reason_not_approximated() {
        let r = parse(
            "Host *\n  ForwardAgent yes\nHost a b\n  HostName x\nHost bastion-hop\n  HostName y\n  ProxyJump bastion\nHost badport\n  Port 99999\nMatch host foo\n  User bar\nInclude ~/.ssh/extra\nHost ok\n",
        );
        assert_eq!(r.hosts.len(), 1);
        assert_eq!(r.hosts[0].alias, "ok");
        assert_eq!(r.hosts[0].host, "ok");
        let patterns: Vec<&str> = r.skipped.iter().map(|s| s.pattern.as_str()).collect();
        assert!(patterns.contains(&"*"));
        assert!(patterns.contains(&"a b"));
        assert!(patterns.contains(&"bastion-hop"));
        assert!(patterns.contains(&"badport"));
        assert!(patterns.iter().any(|p| p.starts_with("Match")));
        assert!(patterns.iter().any(|p| p.starts_with("Include")));
        let hop = r
            .skipped
            .iter()
            .find(|s| s.pattern == "bastion-hop")
            .unwrap();
        assert!(hop.reason.contains("proxyjump"));
    }
}
