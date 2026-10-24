//! Which sites the browser may load.

use std::collections::HashMap;
use std::sync::Mutex;

use url::Url;

/// Allow and deny lists of domains, plus the private-network rule. A domain
/// covers its subdomains; `*.example.com` is the same as `example.com`.
#[derive(Debug, Default)]
pub struct DomainRules {
    allow: Vec<String>,
    deny: Vec<String>,
    allow_private: bool,
    /// Whether each host resolved to a private address, so a page's many
    /// requests need one lookup per host.
    private: Mutex<HashMap<(String, u16), bool>>,
}

impl DomainRules {
    pub fn new(allow: &[String], deny: &[String], allow_private: bool) -> Self {
        let norm = |v: &[String]| {
            v.iter()
                .map(|d| {
                    d.trim()
                        .trim_start_matches("*.")
                        .trim_start_matches('.')
                        .trim_end_matches('.')
                        .to_ascii_lowercase()
                })
                .filter(|d| !d.is_empty())
                .collect()
        };
        Self {
            allow: norm(allow),
            deny: norm(deny),
            allow_private,
            private: Mutex::default(),
        }
    }

    /// Whether `url` may be loaded, without looking up its address. `Err`
    /// says why not.
    pub fn check_static(&self, url: &Url) -> Result<(), String> {
        match url.scheme() {
            "http" | "https" => {}
            // Pages Chrome makes itself.
            "about" | "data" | "blob" => return Ok(()),
            s => return Err(format!("{s}: URLs are not allowed; use http or https")),
        }
        let host = url
            .host_str()
            .ok_or_else(|| format!("{url} has no host"))?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim_end_matches('.')
            .to_ascii_lowercase();
        let covers = |d: &String| host == *d || host.ends_with(&format!(".{d}"));
        if self.deny.iter().any(covers) {
            return Err(format!("{host} is on the browser's deny list"));
        }
        if !self.allow.is_empty() && !self.allow.iter().any(covers) {
            return Err(format!(
                "{host} is not on the browser's allow list ({})",
                self.allow.join(", ")
            ));
        }
        Ok(())
    }

    /// [`check_static`](Self::check_static), then refuse hosts that are or
    /// resolve to private addresses unless those are allowed.
    pub async fn check(&self, url: &Url) -> Result<(), String> {
        self.check_static(url)?;
        if self.allow_private || !matches!(url.scheme(), "http" | "https") {
            return Ok(());
        }
        let host = url.host_str().unwrap_or_default().to_owned();
        let port = url.port_or_known_default().unwrap_or(80);
        let key = (host.clone(), port);
        let cached = self.private.lock().unwrap().get(&key).copied();
        let private = match cached {
            Some(p) => p,
            None => {
                let h = host.clone();
                let p = tokio::task::spawn_blocking(move || {
                    gai_tools::builtin::is_private_host(&h, port)
                })
                .await
                .unwrap_or(true);
                self.private.lock().unwrap().insert(key, p);
                p
            }
        };
        if private {
            return Err(format!(
                "{host} is a private or loopback address; set browser.allow_private_network to allow it"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(allow: &[&str], deny: &[&str]) -> DomainRules {
        let s = |v: &[&str]| v.iter().map(|d| d.to_string()).collect::<Vec<_>>();
        DomainRules::new(&s(allow), &s(deny), false)
    }

    fn ok(r: &DomainRules, url: &str) -> bool {
        r.check_static(&Url::parse(url).unwrap()).is_ok()
    }

    #[test]
    fn domains_cover_subdomains() {
        let r = rules(&["example.com", "*.rust-lang.org"], &["ads.example.com"]);
        assert!(ok(&r, "https://example.com/a"));
        assert!(ok(&r, "https://docs.example.com/"));
        assert!(ok(&r, "https://doc.rust-lang.org/std"));
        assert!(ok(&r, "https://rust-lang.org/"));
        assert!(!ok(&r, "https://ads.example.com/x"));
        assert!(!ok(&r, "https://x.ads.example.com/x"));
        assert!(!ok(&r, "https://notexample.com/"));
        assert!(!ok(&r, "https://example.com.evil.net/"));
    }

    #[test]
    fn schemes() {
        let r = rules(&[], &[]);
        assert!(ok(&r, "http://example.com"));
        assert!(ok(&r, "about:blank"));
        assert!(ok(&r, "data:text/html,hi"));
        assert!(!ok(&r, "file:///etc/passwd"));
        assert!(!ok(&r, "chrome://settings"));
        assert!(!ok(&r, "javascript:alert(1)"));
    }

    #[tokio::test]
    async fn private_hosts_need_permission() {
        let r = rules(&[], &[]);
        for u in [
            "http://127.0.0.1:8080/",
            "http://localhost/",
            "http://[::1]/",
            "http://169.254.169.254/latest",
        ] {
            assert!(r.check(&Url::parse(u).unwrap()).await.is_err(), "{u}");
        }
        let open = DomainRules::new(&[], &[], true);
        assert!(
            open.check(&Url::parse("http://127.0.0.1/").unwrap())
                .await
                .is_ok()
        );
    }
}
