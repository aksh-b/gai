//! Tools that ship with gai.

mod calculator;
mod fs;
mod http;
mod shell;
mod time;

use std::sync::Arc;

use crate::SharedTool;

pub use calculator::calculator;
pub use fs::{fs_edit, fs_list, fs_read, fs_write};
pub use http::{HttpFetch, html_to_text, is_private_host};
pub use shell::Shell;
pub use time::current_time;

pub fn all() -> Vec<SharedTool> {
    vec![
        Arc::new(calculator()),
        Arc::new(current_time()),
        Arc::new(fs_read()),
        Arc::new(fs_write()),
        Arc::new(fs_edit()),
        Arc::new(fs_list()),
        Arc::new(HttpFetch),
        Arc::new(Shell),
    ]
}
