//! A web browser for agents.
//!
//! The [`chrome`] backend drives headless Chrome (or Chromium) over the
//! DevTools protocol. Each run gets its own browser context, so cookies and
//! logins never leak between runs, and the context is closed once the run
//! stops using it.
//!
//! [`tools`] gives agents `browser_navigate`, `browser_read` (a compact
//! outline of the page with numbered elements, or its readable text),
//! `browser_click`, `browser_type`, `browser_submit`, `browser_extract` and
//! `browser_screenshot`. Every request the page makes, redirects and
//! subresources included, is checked against [`DomainRules`], and submitting
//! a form goes through `browser_submit`, which needs approval by default.

#[cfg(feature = "chrome")]
pub mod chrome;
mod rules;
#[cfg(feature = "chrome")]
mod tools;

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use rules::DomainRules;
#[cfg(feature = "chrome")]
pub use tools::{ImageSink, tools};

/// ```toml
/// [browser]
/// enabled = true                  # the tools exist only when Chrome is found
/// # executable = "/usr/bin/chromium"   # default: search PATH
/// allow_domains = []              # empty: any public site
/// deny_domains = ["facebook.com"]
/// allow_private_network = false   # localhost and private addresses
/// timeout_secs = 30
/// idle_secs = 300
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserSettings {
    /// Offer the browser tools when a Chrome or Chromium is found.
    pub enabled: bool,
    /// The Chrome/Chromium binary. Defaults to the first of `chromium`,
    /// `chromium-browser`, `google-chrome`, `google-chrome-stable` and
    /// `chrome` on `PATH` (or `GAI_BROWSER_EXECUTABLE`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable: Option<PathBuf>,
    /// Run without a window. Turn off to watch what the agent does.
    pub headless: bool,
    /// Sites the browser may load (`example.com` also covers its
    /// subdomains). Empty means any site not denied.
    pub allow_domains: Vec<String>,
    /// Sites the browser never loads, even when allowed.
    pub deny_domains: Vec<String>,
    /// Let pages reach localhost and private networks.
    pub allow_private_network: bool,
    /// Longest wait for one page load or action.
    pub timeout_secs: u64,
    /// A run's browser context is closed after this long unused.
    pub idle_secs: u64,
    /// Most runs with an open browser context at once.
    pub max_contexts: usize,
    /// Window size, which is also the screenshot size.
    pub width: u32,
    pub height: u32,
    /// Extra Chrome command-line flags.
    pub args: Vec<String>,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            executable: None,
            headless: true,
            allow_domains: Vec::new(),
            deny_domains: Vec::new(),
            allow_private_network: false,
            timeout_secs: 30,
            idle_secs: 300,
            max_contexts: 4,
            width: 1280,
            height: 800,
            args: Vec::new(),
        }
    }
}

impl BrowserSettings {
    pub fn rules(&self) -> DomainRules {
        DomainRules::new(
            &self.allow_domains,
            &self.deny_domains,
            self.allow_private_network,
        )
    }

    /// The Chrome binary to launch, if one is configured or on `PATH`.
    pub fn find_executable(&self) -> Option<PathBuf> {
        if let Some(p) = &self.executable {
            return p.is_file().then(|| p.clone());
        }
        if let Some(p) = std::env::var_os("GAI_BROWSER_EXECUTABLE").map(PathBuf::from) {
            return p.is_file().then_some(p);
        }
        let path = std::env::var_os("PATH")?;
        const NAMES: [&str; 5] = [
            "chromium",
            "chromium-browser",
            "google-chrome",
            "google-chrome-stable",
            "chrome",
        ];
        std::env::split_paths(&path).find_map(|dir| {
            NAMES.iter().find_map(|n| {
                let p = dir.join(n);
                let exe = p.with_extension("exe");
                if p.is_file() {
                    Some(p)
                } else {
                    (cfg!(windows) && exe.is_file()).then_some(exe)
                }
            })
        })
    }
}
