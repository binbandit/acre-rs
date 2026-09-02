//! Reads the host, owner, and repository out of a Git remote URL.

#[derive(Debug, Clone)]
pub struct HostedRemote {
    pub host: String,
    pub owner: String,
    pub repo: String,
}

pub fn parse_hosted_remote(remote_url: &str) -> Option<HostedRemote> {
    let sanitized = remote_url.trim().trim_end_matches('/').trim_end_matches(".git");
    if !sanitized.contains("://") {
        if let Some((left, right)) = sanitized.split_once(':') {
            let host = left.rsplit('@').next()?.to_owned();
            let mut parts: Vec<&str> = right.split('/').filter(|part| !part.is_empty()).collect();
            if parts.len() >= 2 {
                let repo = parts.pop()?.to_owned();
                return Some(HostedRemote {
                    host,
                    owner: parts.join("/"),
                    repo,
                });
            }
        }
    }
    let without_scheme = sanitized.split_once("://")?.1;
    let (host, path) = without_scheme.split_once('/')?;
    let mut parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }
    let repo = parts.pop()?.to_owned();
    Some(HostedRemote {
        host: host
            .split('@')
            .next_back()
            .unwrap_or(host)
            .split(':')
            .next()
            .unwrap_or(host)
            .to_owned(),
        owner: parts.join("/"),
        repo,
    })
}
