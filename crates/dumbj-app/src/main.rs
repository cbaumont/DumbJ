//! Usage: `dumbj [PATH]...`. A folder is opened as the project and files are opened in
//! tabs; with only files, the first file's folder becomes the project.
//! Set `DUMBJ_LOG` to a `tracing` filter (for example `debug` or `dumbj_ui=trace`) for
//! more log output on stderr. `DUMBJ_SCROLL_BENCHMARK=1` scrolls through the opened file,
//! logs frame-time statistics and exits.

use std::path::PathBuf;

use anyhow::Context as _;
use tracing_subscriber::EnvFilter;

fn init_logging() {
    let filter = EnvFilter::try_from_env("DUMBJ_LOG")
        .unwrap_or_else(|_| EnvFilter::new("warn,dumbj_ui=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

fn main() -> anyhow::Result<()> {
    init_logging();
    let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    for path in &paths {
        anyhow::ensure!(path.exists(), "{} does not exist", path.display());
    }
    let scroll_benchmark = std::env::var_os("DUMBJ_SCROLL_BENCHMARK").is_some();
    dumbj_ui::run(dumbj_ui::RunOptions {
        paths,
        scroll_benchmark,
    })
    .context("DumbJ could not start")
}
