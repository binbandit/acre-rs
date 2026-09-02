//! Reads the host, owner, and repository out of a Git remote URL.

#[derive(Debug, Clone)]
pub struct HostedRemote {
    pub host: String,
    pub owner: String,
    pub repo: String,
}

pub fn parse_hosted_remote(remote_url: &str) -> Option<HostedRemote> {
    let sanitized = remote_url.trim().trim_end_matches('/').trim_end_matches(".git");
    // scp-style git@host:owner/repo has no scheme; handle it before the URL form.
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
        // Strip user@ and :port from the authority; only the hostname identifies the forge.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ssh_and_https_remotes() {
        for url in [
            "git@github.com:acme/widgets.git",
            "https://github.com/acme/widgets",
            "ssh://git@github.com/acme/widgets.git",
        ] {
            let remote = parse_hosted_remote(url).expect(url);
            assert_eq!(
                (remote.host.as_str(), remote.owner.as_str(), remote.repo.as_str()),
                ("github.com", "acme", "widgets")
            );
        }
        assert!(parse_hosted_remote("/srv/git/widgets.git").is_none());
    }
}
