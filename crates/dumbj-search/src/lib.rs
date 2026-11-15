use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use thiserror::Error;

/// Files larger than this are skipped: they are generated or data, and reading them would
/// stall the search.
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// How much of a file is checked for NUL bytes to tell binary files from text.
const BINARY_SNIFF_BYTES: usize = 8192;
/// Longer lines are cut so that one minified file cannot fill the results.
const MAX_LINE_CHARS: usize = 300;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub query: String,
    pub regex: bool,
    pub match_case: bool,
    pub whole_word: bool,
    /// Comma or space separated globs such as `*.java, *.kt`. A leading `!` excludes.
    pub file_mask: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub path: PathBuf,
    /// Zero-based.
    pub line: usize,
    /// The line without its leading whitespace, cut to a reasonable length.
    pub text: String,
    /// Byte ranges of the matches inside `text`.
    pub ranges: Vec<Range<usize>>,
    /// Byte offset in the file of the first match on the line.
    pub offset: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// In path order, then line order.
    pub matches: Vec<Match>,
    /// More matches exist than `matches` holds.
    pub capped: bool,
    pub files_searched: usize,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SearchError {
    #[error("invalid pattern: {0}")]
    Pattern(String),
    #[error("invalid file mask: {0}")]
    Mask(String),
    #[error("the search was cancelled")]
    Cancelled,
}

struct Mask {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
}

impl Mask {
    fn parse(text: &str) -> Result<Self, SearchError> {
        let (mut include, mut exclude) = (GlobSetBuilder::new(), GlobSetBuilder::new());
        let (mut includes, mut excludes) = (0, 0);
        for part in text.split([',', ' ', ';']).filter(|part| !part.is_empty()) {
            let (builder, count, pattern) = match part.strip_prefix('!') {
                Some(rest) => (&mut exclude, &mut excludes, rest),
                None => (&mut include, &mut includes, part),
            };
            // A pattern without a slash applies to the file name at any depth.
            let pattern = if pattern.contains('/') {
                pattern.to_owned()
            } else {
                format!("**/{pattern}")
            };
            let glob = Glob::new(&pattern).map_err(|error| SearchError::Mask(error.to_string()))?;
            builder.add(glob);
            *count += 1;
        }
        let build = |builder: GlobSetBuilder, count: usize| {
            if count == 0 {
                return Ok(None);
            }
            builder
                .build()
                .map(Some)
                .map_err(|error| SearchError::Mask(error.to_string()))
        };
        Ok(Self {
            include: build(include, includes)?,
            exclude: build(exclude, excludes)?,
        })
    }

    fn accepts(&self, relative: &Path) -> bool {
        self.include
            .as_ref()
            .is_none_or(|include| include.is_match(relative))
            && self
                .exclude
                .as_ref()
                .is_none_or(|exclude| !exclude.is_match(relative))
    }
}

fn build_regex(options: &SearchOptions) -> Result<Regex, SearchError> {
    let mut pattern = if options.regex {
        options.query.clone()
    } else {
        regex::escape(&options.query)
    };
    if options.whole_word {
        pattern = format!(r"\b(?:{pattern})\b");
    }
    RegexBuilder::new(&pattern)
        .case_insensitive(!options.match_case)
        .build()
        .map_err(|error| SearchError::Pattern(error.to_string()))
}

/// Searches the text files under `scope` (honouring `.gitignore`, skipping hidden, binary
/// and huge files) for `options.query`, line by line. `root` only anchors the file mask.
///
/// Stops with `capped` once more than `max_matches` lines match. Polls `cancelled`
/// and returns `Cancelled` when it is set.
pub fn find_in_files(
    root: &Path,
    scope: &Path,
    options: &SearchOptions,
    max_matches: usize,
    cancelled: &AtomicBool,
) -> Result<Outcome, SearchError> {
    if options.query.is_empty() {
        return Ok(Outcome::default());
    }
    let pattern = build_regex(options)?;
    let mask = Mask::parse(&options.file_mask)?;

    let mut files: Vec<PathBuf> = Vec::new();
    let walker = WalkBuilder::new(scope)
        .require_git(false)
        .sort_by_file_path(Ord::cmp)
        .build();
    for entry in walker.flatten() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(SearchError::Cancelled);
        }
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.into_path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if mask.accepts(relative) {
            files.push(path);
        }
    }

    let next = AtomicUsize::new(0);
    let found = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Vec<Match>)>> = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(2, usize::from);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                // Files are claimed in order, so when the cap is reached every earlier file
                // is finished or in progress, and the merge below is exact.
                loop {
                    if cancelled.load(Ordering::Relaxed)
                        || found.load(Ordering::Relaxed) > max_matches
                    {
                        return;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(index) else { return };
                    let hits = search_file(path, &pattern, cancelled);
                    if hits.is_empty() {
                        continue;
                    }
                    found.fetch_add(hits.len(), Ordering::Relaxed);
                    if let Ok(mut results) = results.lock() {
                        results.push((index, hits));
                    }
                }
            });
        }
    });
    if cancelled.load(Ordering::Relaxed) {
        return Err(SearchError::Cancelled);
    }

    let mut results = results.into_inner().unwrap_or_default();
    results.sort_by_key(|(index, _)| *index);
    let mut matches: Vec<Match> = results.into_iter().flat_map(|(_, found)| found).collect();
    let capped = matches.len() > max_matches;
    matches.truncate(max_matches);
    Ok(Outcome {
        matches,
        capped,
        files_searched: next.load(Ordering::Relaxed).min(files.len()),
    })
}

fn search_file(path: &Path, pattern: &Regex, cancelled: &AtomicBool) -> Vec<Match> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Vec::new();
    };
    if metadata.len() > MAX_FILE_BYTES {
        return Vec::new();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    if bytes.iter().take(BINARY_SNIFF_BYTES).any(|byte| *byte == 0) {
        return Vec::new();
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Vec::new();
    };
    let mut matches = Vec::new();
    let mut line_start = 0;
    for (line, raw) in text.split('\n').enumerate() {
        if line % 1024 == 0 && cancelled.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let next_start = line_start + raw.len() + 1;
        let content = raw.strip_suffix('\r').unwrap_or(raw);
        let ranges: Vec<Range<usize>> = pattern
            .find_iter(content)
            .filter(|found| !found.is_empty())
            .map(|found| found.range())
            .collect();
        if let Some(first) = ranges.first() {
            matches.push(line_match(
                path,
                line,
                content,
                &ranges,
                line_start + first.start,
            ));
        }
        line_start = next_start;
    }
    matches
}

fn line_match(
    path: &Path,
    line: usize,
    content: &str,
    ranges: &[Range<usize>],
    offset: usize,
) -> Match {
    let indent = content.len() - content.trim_start().len();
    let trimmed = &content[indent..];
    let end = trimmed
        .char_indices()
        .nth(MAX_LINE_CHARS)
        .map_or(trimmed.len(), |(index, _)| index);
    let ranges = ranges
        .iter()
        .filter_map(|range| {
            let start = range.start.saturating_sub(indent);
            let stop = range.end.saturating_sub(indent).min(end);
            (start < stop).then_some(start..stop)
        })
        .collect();
    Match {
        path: path.to_path_buf(),
        line,
        text: trimmed[..end].to_owned(),
        ranges,
        offset,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    struct Project(PathBuf);

    impl Project {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("dumbj-search-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self, relative: &str, content: impl AsRef<[u8]>) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        fn search(&self, query: &str, tweak: impl FnOnce(&mut SearchOptions)) -> Outcome {
            let mut options = SearchOptions {
                query: query.to_owned(),
                match_case: true,
                ..SearchOptions::default()
            };
            tweak(&mut options);
            find_in_files(&self.0, &self.0, &options, 100, &AtomicBool::new(false)).unwrap()
        }

        fn hits(&self, query: &str, tweak: impl FnOnce(&mut SearchOptions)) -> Vec<String> {
            self.search(query, tweak)
                .matches
                .iter()
                .map(|found| {
                    let name = found.path.strip_prefix(&self.0).unwrap();
                    format!("{}:{}:{}", name.display(), found.line + 1, found.text)
                })
                .collect()
        }
    }

    impl Drop for Project {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn finds_literal_text_in_path_order() {
        let project = Project::new("literal");
        project.file("b/B.java", "class B {\n    int total;\n}\n");
        project.file("a/A.kt", "val total = 1\nval other = total + 1\n");
        assert_eq!(
            project.hits("total", |_| {}),
            [
                "a/A.kt:1:val total = 1",
                "a/A.kt:2:val other = total + 1",
                "b/B.java:2:int total;"
            ]
        );
    }

    #[test]
    fn a_literal_query_is_not_a_regex() {
        let project = Project::new("escape");
        project.file("A.txt", "a.b\naxb\n");
        assert_eq!(project.hits("a.b", |_| {}), ["A.txt:1:a.b"]);
        assert_eq!(
            project.hits("a.b", |options| options.regex = true),
            ["A.txt:1:a.b", "A.txt:2:axb"]
        );
    }

    #[test]
    fn case_and_whole_word() {
        let project = Project::new("case");
        project.file("A.txt", "Cart\ncart\ncarts\nmy cart!\n");
        assert_eq!(project.hits("cart", |_| {}).len(), 3);
        assert_eq!(
            project
                .hits("cart", |options| options.match_case = false)
                .len(),
            4
        );
        assert_eq!(
            project.hits("cart", |options| options.whole_word = true),
            ["A.txt:2:cart", "A.txt:4:my cart!"]
        );
    }

    #[test]
    fn whole_word_applies_to_a_regex_alternation() {
        let project = Project::new("word-regex");
        project.file("A.txt", "cat\ncatalog\ndog\ndogs\n");
        assert_eq!(
            project.hits("cat|dog", |options| {
                options.regex = true;
                options.whole_word = true;
            }),
            ["A.txt:1:cat", "A.txt:3:dog"]
        );
    }

    #[test]
    fn an_invalid_regex_is_an_error() {
        let project = Project::new("bad");
        let options = SearchOptions {
            query: "(".to_owned(),
            regex: true,
            ..SearchOptions::default()
        };
        let result = find_in_files(
            &project.0,
            &project.0,
            &options,
            10,
            &AtomicBool::new(false),
        );
        assert!(matches!(result, Err(SearchError::Pattern(_))));
    }

    #[test]
    fn file_masks_include_and_exclude() {
        let project = Project::new("mask");
        project.file("src/A.java", "needle\n");
        project.file("src/B.kt", "needle\n");
        project.file("src/B.test.kt", "needle\n");
        project.file("notes.md", "needle\n");
        assert_eq!(
            project.hits("needle", |options| options.file_mask =
                "*.java, *.kt".to_owned()),
            [
                "src/A.java:1:needle",
                "src/B.kt:1:needle",
                "src/B.test.kt:1:needle"
            ]
        );
        assert_eq!(
            project.hits("needle", |options| options.file_mask =
                "*.kt !*.test.kt".to_owned()),
            ["src/B.kt:1:needle"]
        );
        assert_eq!(
            project.hits("needle", |options| options.file_mask =
                "src/*.java".to_owned()),
            ["src/A.java:1:needle"]
        );
        assert!(matches!(
            find_in_files(
                &project.0,
                &project.0,
                &SearchOptions {
                    query: "x".to_owned(),
                    file_mask: "[".to_owned(),
                    ..SearchOptions::default()
                },
                10,
                &AtomicBool::new(false)
            ),
            Err(SearchError::Mask(_))
        ));
    }

    #[test]
    fn honours_gitignore_and_skips_hidden_binary_and_huge_files() {
        let project = Project::new("skips");
        project.file(".gitignore", "build/\n");
        project.file("build/Out.java", "needle\n");
        project.file(".hidden/H.java", "needle\n");
        project.file("bin.dat", b"needle\0\0\0".as_slice());
        project.file("latin1.txt", b"needle \xe9\n".as_slice());
        let huge = format!("needle\n{}", "x".repeat(6 * 1024 * 1024));
        project.file("huge.txt", huge);
        project.file("src/Keep.java", "needle\n");
        assert_eq!(project.hits("needle", |_| {}), ["src/Keep.java:1:needle"]);
    }

    #[test]
    fn can_search_below_a_directory_only() {
        let project = Project::new("scope");
        project.file("a/A.txt", "needle\n");
        project.file("b/B.txt", "needle\n");
        let options = SearchOptions {
            query: "needle".to_owned(),
            ..SearchOptions::default()
        };
        let outcome = find_in_files(
            &project.0,
            &project.0.join("b"),
            &options,
            10,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(outcome.matches.len(), 1);
        assert!(outcome.matches[0].path.ends_with("b/B.txt"));
    }

    #[test]
    fn reports_ranges_offsets_and_trims_indentation() {
        let project = Project::new("ranges");
        project.file("A.java", "first\n\t  int ab = ab + 1;\r\n");
        let outcome = project.search("ab", |_| {});
        let found = &outcome.matches[0];
        assert_eq!(found.text, "int ab = ab + 1;");
        assert_eq!(found.ranges, [4..6, 9..11]);
        assert_eq!(found.line, 1);
        assert_eq!(found.offset, "first\n\t  int ".len());
    }

    #[test]
    fn long_lines_are_cut() {
        let project = Project::new("long");
        project.file("A.txt", format!("{}needle\n", "x".repeat(1000)));
        let outcome = project.search("needle", |_| {});
        assert_eq!(outcome.matches.len(), 1);
        assert_eq!(outcome.matches[0].text.len(), MAX_LINE_CHARS);
        assert_eq!(outcome.matches[0].ranges, Vec::<Range<usize>>::new());
    }

    #[test]
    fn caps_the_results_and_says_so() {
        let project = Project::new("cap");
        for file in 0..5 {
            project.file(&format!("f{file}.txt"), "hit\n".repeat(10));
        }
        let options = SearchOptions {
            query: "hit".to_owned(),
            ..SearchOptions::default()
        };
        let outcome = find_in_files(
            &project.0,
            &project.0,
            &options,
            25,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(outcome.capped);
        assert_eq!(outcome.matches.len(), 25);
        assert!(
            outcome.matches[..10]
                .iter()
                .all(|m| m.path.ends_with("f0.txt"))
        );
        assert!(
            outcome.matches[20..]
                .iter()
                .all(|m| m.path.ends_with("f2.txt"))
        );
        let all = find_in_files(
            &project.0,
            &project.0,
            &options,
            50,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!all.capped);
        assert_eq!(all.matches.len(), 50);
    }

    #[test]
    fn cancelling_stops_the_search() {
        let project = Project::new("cancel");
        project.file("A.txt", "needle\n");
        let options = SearchOptions {
            query: "needle".to_owned(),
            ..SearchOptions::default()
        };
        let result = find_in_files(&project.0, &project.0, &options, 10, &AtomicBool::new(true));
        assert_eq!(result, Err(SearchError::Cancelled));
    }

    #[test]
    fn an_empty_query_finds_nothing() {
        let project = Project::new("empty");
        project.file("A.txt", "text\n");
        assert_eq!(project.search("", |_| {}), Outcome::default());
    }

    #[test]
    fn a_regex_that_matches_nothing_but_empty_strings_finds_nothing() {
        let project = Project::new("empty-match");
        project.file("A.txt", "abc\n");
        assert_eq!(
            project.hits("x*", |options| options.regex = true),
            Vec::<String>::new()
        );
    }
}
