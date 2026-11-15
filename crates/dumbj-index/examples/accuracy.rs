//! The dumb-mode accuracy benchmark (BRIEF §4.3.1): runs Go to Declaration on hand-checked
//! cases in the pinned corpus and reports how often it is right.
//!
//! Usage: `cargo run --release -p dumbj-index --example accuracy -- [CORPUS_DIR]`
//! (fetch the corpus first with `scripts/fetch-corpus.sh`; the default directory is
//! `$XDG_CACHE_HOME/dumbj/corpus`). Cases live in `corpus/expectations/<repo>.toml`:
//!
//! ```toml
//! [[case]]
//! file = "path/in/repo/Caller.kt"
//! at = "val x = fo§o(bar)"      # occurs exactly once in the file; § marks the caret
//! target = "path/in/repo/Decl.kt:42"   # file and 1-based line of the declared name,
//!                                       # or "none" when the declaration is not in the repo
//! ```
//!
//! Outcomes: `correct` (jumped to the target, or found nothing for "none"), `chooser`
//! (offered a chooser that contains the target), `missed` (found nothing, or a chooser
//! without the target) and `wrong` (jumped somewhere else: the outcome that must stay rare).
//! The exit code is non-zero only when a case file is malformed.

#![expect(clippy::print_stdout, reason = "a command-line report")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use dumbj_index::{Decision, Index, Site, index_project, resolve};
use dumbj_syntax::Language;
use dumbj_text::{Buffer, ByteOffset};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseFile {
    case: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    file: String,
    at: String,
    target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Correct,
    Chooser,
    Missed,
    Wrong,
}

#[derive(Default)]
struct Tally {
    counts: BTreeMap<Outcome, usize>,
}

impl Tally {
    fn add(&mut self, outcome: Outcome) {
        *self.counts.entry(outcome).or_default() += 1;
    }

    fn get(&self, outcome: Outcome) -> usize {
        self.counts.get(&outcome).copied().unwrap_or(0)
    }

    fn row(&self, label: &str) -> String {
        let total: usize = self.counts.values().sum();
        #[expect(clippy::cast_precision_loss, reason = "report percentages")]
        let percent = |count: usize| 100.0 * count as f64 / total.max(1) as f64;
        format!(
            "{label:<28} {total:>5}  correct {:>3} ({:>5.1}%)  chooser {:>3}  missed {:>3}  wrong {:>3} ({:>4.1}%)",
            self.get(Outcome::Correct),
            percent(self.get(Outcome::Correct)),
            self.get(Outcome::Chooser),
            self.get(Outcome::Missed),
            self.get(Outcome::Wrong),
            percent(self.get(Outcome::Wrong)),
        )
    }
}

fn run_case(
    index: &Index,
    root: &Path,
    case: &Case,
) -> Result<(Language, Outcome, String), String> {
    let path = root.join(&case.file);
    let source =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", case.file))?;
    let plain = case.at.replacen('§', "", 1);
    let caret = case
        .at
        .find('§')
        .ok_or_else(|| format!("{}: no § in `{}`", case.file, case.at))?;
    if source.matches(&plain).count() != 1 {
        return Err(format!("{}: `{plain}` must occur exactly once", case.file));
    }
    let offset = source.find(&plain).unwrap_or(0) + caret;
    let language =
        Language::from_path(&path).ok_or_else(|| format!("{}: unknown language", case.file))?;
    let file = index
        .file_by_path(&path)
        .ok_or_else(|| format!("{}: not indexed", case.file))?;
    let buffer = Buffer::new(&source);
    let tree = dumbj_syntax::parse(language, &buffer, || false)
        .ok()
        .flatten()
        .ok_or_else(|| format!("{}: cannot parse", case.file))?;
    let resolution = resolve(
        Site {
            index,
            file,
            tree: &tree,
            buffer: &buffer,
        },
        ByteOffset(offset),
    );

    let describe = |target: &dumbj_index::Target| {
        let relative = target
            .path
            .strip_prefix(root)
            .unwrap_or(&target.path)
            .display()
            .to_string();
        let text = if target.path == path {
            Some(source.clone())
        } else {
            std::fs::read_to_string(&target.path).ok()
        };
        let line = text
            .and_then(|text| {
                text.get(..target.range.start.get())
                    .map(|before| before.matches('\n').count() + 1)
            })
            .unwrap_or(0);
        format!("{relative}:{line}")
    };
    let (decision, found): (Decision, Vec<String>) = match &resolution {
        Some(resolution) => (
            resolution.decision,
            resolution.targets.iter().map(describe).collect(),
        ),
        None => (Decision::NotFound, Vec::new()),
    };
    let expects_nothing = case.target == "none";
    let hit = found.contains(&case.target);
    let outcome = match decision {
        Decision::NotFound if expects_nothing => Outcome::Correct,
        Decision::Jump if found.first() == Some(&case.target) => Outcome::Correct,
        Decision::Jump => Outcome::Wrong,
        Decision::Choose if hit => Outcome::Chooser,
        Decision::NotFound | Decision::Choose => Outcome::Missed,
    };
    let got = match decision {
        Decision::NotFound => "nothing".to_owned(),
        Decision::Jump => format!("jump to {}", found.first().cloned().unwrap_or_default()),
        Decision::Choose => format!("chooser of {}: {}", found.len(), found.join(", ")),
    };
    Ok((language, outcome, got))
}

fn default_corpus() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")));
    cache.unwrap_or_default().join("dumbj/corpus")
}

fn main() -> std::process::ExitCode {
    let corpus = std::env::args_os()
        .nth(1)
        .map_or_else(default_corpus, PathBuf::from);
    let expectations = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/expectations");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&expectations)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();

    let mut by_language: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut total = Tally::default();
    let mut malformed = 0;
    for file in files {
        let repo = file
            .file_stem()
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
        let root = corpus.join(&repo);
        let cases = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| toml::from_str::<CaseFile>(&text).ok());
        let (Some(cases), true) = (cases, root.is_dir()) else {
            println!(
                "{repo}: cannot read cases, or the repository is not fetched at {}",
                root.display()
            );
            malformed += 1;
            continue;
        };
        let Some(index) = index_project(&root, &AtomicBool::new(false), |_| {}) else {
            continue;
        };
        let mut tally = Tally::default();
        for case in &cases.case {
            match run_case(&index, &root, case) {
                Ok((language, outcome, got)) => {
                    tally.add(outcome);
                    total.add(outcome);
                    by_language.entry(language.name()).or_default().add(outcome);
                    if outcome != Outcome::Correct {
                        println!(
                            "  {outcome:?}: {} `{}`\n      expected {}, got {got}",
                            case.file, case.at, case.target
                        );
                    }
                }
                Err(problem) => {
                    println!("  malformed case: {problem}");
                    malformed += 1;
                }
            }
        }
        println!("{}", tally.row(&repo));
    }
    println!();
    for (language, tally) in &by_language {
        println!("{}", tally.row(language));
    }
    println!("{}", total.row("all"));
    if malformed > 0 {
        println!("{malformed} malformed case(s)");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
