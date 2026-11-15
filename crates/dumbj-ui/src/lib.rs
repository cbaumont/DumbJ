mod assets;
mod editor;
mod editor_model;
mod file_tree;
mod keys;
mod line_edit;
mod text_input;
mod theme;
mod tree_model;
mod workspace;

use std::path::PathBuf;

use dumbj_actions::builtin::{self, BuiltinError, Platform};
use dumbj_settings::{Theme, ThemeError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UiError {
    #[error("the built-in theme is invalid: {0}")]
    Theme(#[from] ThemeError),
    #[error("the built-in keymap is invalid: {0}")]
    Keymap(#[from] BuiltinError),
}

#[derive(Debug, Default)]
pub struct RunOptions {
    /// Paths from the command line: the first folder is opened as the project and files
    /// are opened in tabs. With only files, the first file's folder becomes the project.
    pub paths: Vec<PathBuf>,
    pub scroll_benchmark: bool,
}

pub fn run(options: RunOptions) -> Result<(), UiError> {
    let theme = Theme::catppuccin_mocha()?;
    let platform = Platform::current();
    let bindings = builtin::default_bindings(platform)?;
    tracing::debug!(
        keymap = bindings.keymap_name(),
        unmapped = bindings.unmapped_intellij_actions().len(),
        "keymap loaded; IntelliJ actions without a DumbJ action are ignored"
    );
    let RunOptions {
        paths,
        scroll_benchmark,
    } = options;
    let (folder, files) = workspace::resolve_cli_paths(&paths);

    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(move |cx| {
            if let Err(error) = cx.text_system().add_fonts(assets::fonts()) {
                tracing::error!(%error, "cannot load the bundled fonts");
            }
            cx.set_global(theme::ActiveTheme(theme));
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let open = workspace::OpenOptions {
                folder,
                files,
                scroll_benchmark,
            };
            workspace::open_window(cx, bindings, platform.shortcut_style(), open);
            cx.activate(true);
        });
    Ok(())
}
