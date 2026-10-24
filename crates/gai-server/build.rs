//! Points the embedded dashboard at `web/dist`, or at a placeholder page
//! when the dashboard has not been built (`npm run build` in `web/`), so the
//! server always compiles.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dist = manifest.join("../../web/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    println!("cargo:rerun-if-env-changed=GAI_UI_EMBED_DIR");
    let dir = match std::env::var_os("GAI_UI_EMBED_DIR").map(PathBuf::from) {
        Some(d) => d,
        None if dist.join("index.html").is_file() => dist,
        None => {
            let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("no-ui");
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(out.join("index.html"), PLACEHOLDER).unwrap();
            out
        }
    };
    let dir = dir.canonicalize().unwrap_or(dir);
    println!("cargo:rustc-env=GAI_UI_EMBED_DIR={}", dir.display());
}

const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="gai-placeholder" content="1"><title>gai</title>
<style>body{font:16px/1.5 system-ui,sans-serif;max-width:40rem;margin:4rem auto;padding:0 1rem;color:#222;background:#fff}
code,pre{background:#f2f2f2;padding:.1rem .3rem;border-radius:4px}pre{padding:.75rem}a{color:#2459c4}
@media (prefers-color-scheme:dark){body{color:#ddd;background:#111}code,pre{background:#222}a{color:#8ab4ff}}</style></head>
<body><h1>gai is running</h1>
<p>This binary was built without the dashboard. The API works: see <a href="/docs">/docs</a>.</p>
<p>To include the dashboard, build it and then rebuild gai:</p>
<pre>cd web &amp;&amp; npm install &amp;&amp; npm run build &amp;&amp; cd ..
cargo build --release -p gai-cli</pre>
<p>Or serve a dashboard build from disk with <code>gai serve --ui web/dist</code>.</p></body></html>
"#;
