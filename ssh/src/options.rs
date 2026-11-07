use crate::Result;
use alloc::{
    format,
    string::{String, ToString},
};

pub const DEFAULT_PORT: u16 = 22;
pub const DEFAULT_IDENTITY: &str = ".ssh/id_ed25519";
pub const KNOWN_HOSTS: &str = ".ssh/known_hosts";
pub const USAGE: &str =
    "usage: ssh [-p port] [-l user] [-i identity] [--accept-new] [user@]host[:port]\r\n";

#[derive(Debug, Clone)]
pub struct Options {
    pub host: String,
    pub host_token: String,
    pub port: u16,
    pub user: String,
    pub identity: String,
    pub accept_new: bool,
}
impl Options {
    /// None means help, without terminating the resident application.
    pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Self>> {
        let mut args = args.into_iter();
        let (mut port, mut user, mut target) = (None, None, None);
        let mut identity = DEFAULT_IDENTITY.to_string();
        let mut accept_new = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-p" => port = Some(parse_port(&args.next().ok_or("ssh: -p requires a port")?)?),
                "-l" => user = Some(args.next().ok_or("ssh: -l requires a user")?),
                "-i" => identity = args.next().ok_or("ssh: -i requires a path")?,
                "--accept-new" => accept_new = true,
                "-h" | "--help" | "help" => return Ok(None),
                _ if arg.starts_with('-') => return Err(format!("ssh: unsupported option {arg}")),
                _ if target.is_none() => target = Some(arg),
                _ => return Err("ssh: only one destination is supported".into()),
            }
        }
        let target = target.ok_or("ssh: missing destination")?;
        let (target_user, endpoint) = target
            .rsplit_once('@')
            .map_or((None, target.as_str()), |(u, h)| (Some(u.to_string()), h));
        let (host, endpoint_port) = split_endpoint(endpoint)?;
        let port = port.or(endpoint_port).unwrap_or(DEFAULT_PORT);
        let user = user.or(target_user).unwrap_or_else(|| "root".into());
        if user.is_empty() {
            return Err("ssh: empty user".into());
        }
        if identity.is_empty() {
            return Err("ssh: empty identity path".into());
        }
        let host_token = if port == DEFAULT_PORT {
            host.clone()
        } else {
            format!("[{host}]:{port}")
        };
        Ok(Some(Self {
            host,
            host_token,
            port,
            user,
            identity,
            accept_new,
        }))
    }
}
fn parse_port(value: &str) -> Result<u16> {
    value
        .parse()
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| format!("ssh: invalid port {value:?}"))
}
fn split_endpoint(endpoint: &str) -> Result<(String, Option<u16>)> {
    if let Some(bracketed) = endpoint.strip_prefix('[') {
        let (host, rest) = bracketed
            .split_once(']')
            .ok_or("ssh: missing ] in IPv6 destination")?;
        if host.is_empty() {
            return Err("ssh: empty host".into());
        }
        let port = if rest.is_empty() {
            None
        } else {
            Some(parse_port(
                rest.strip_prefix(':')
                    .ok_or("ssh: expected :port after ]")?,
            )?)
        };
        return Ok((host.into(), port));
    }
    match endpoint.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            if host.is_empty() {
                return Err("ssh: empty host".into());
            }
            Ok((host.into(), Some(parse_port(port)?)))
        }
        _ if endpoint.is_empty() => Err("ssh: empty host".into()),
        _ => Ok((endpoint.into(), None)),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_accepts_shell2_and_ipv6_forms() {
        assert_eq!(
            split_endpoint("192.168.178.94:4548").unwrap(),
            ("192.168.178.94".into(), Some(4548))
        );
        assert_eq!(
            split_endpoint("192.168.178.111").unwrap(),
            ("192.168.178.111".into(), None)
        );
        assert_eq!(
            split_endpoint("[fe80::1]:2222").unwrap(),
            ("fe80::1".into(), Some(2222))
        );
        assert_eq!(split_endpoint("fe80::1").unwrap(), ("fe80::1".into(), None));
    }
    #[test]
    fn endpoint_rejects_empty_or_invalid_port() {
        for s in ["192.168.178.111:", "[]", ":22", "[::1]oops", "host:0"] {
            assert!(split_endpoint(s).is_err(), "{s}")
        }
    }
    #[test]
    fn options_keep_precedence_and_help_is_not_process_exit() {
        let args = ["-p", "2222", "-l", "override", "user@host:44"].map(String::from);
        let o = Options::parse_args(args).unwrap().unwrap();
        assert_eq!(
            (o.user.as_str(), o.port, o.host_token.as_str()),
            ("override", 2222, "[host]:2222")
        );
        assert!(Options::parse_args(["help".into()]).unwrap().is_none());
    }
}
