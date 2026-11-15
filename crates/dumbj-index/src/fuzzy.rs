//! Fuzzy matching in the IDEA style: the pattern's characters must appear in order, an
//! upper-case pattern letter must land on a word start or a capital ("CamelHumps": `SE`
//! finds `SearchEverywhere`), and matches at word starts and in a row score higher.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub score: i32,
    /// The byte offsets in the candidate of the matched characters, ascending.
    pub positions: Vec<usize>,
}

const START_BONUS: i32 = 12;
const WORD_START_BONUS: i32 = 8;
const CONSECUTIVE_BONUS: i32 = 6;
const CASE_BONUS: i32 = 1;
const MATCH_SCORE: i32 = 2;
const GAP_PENALTY: i32 = 1;
const BREAK_PENALTY: i32 = 3;
const PREFIX_BONUS: i32 = 20;
const EXACT_BONUS: i32 = 40;
const NONE: i32 = i32::MIN / 2;

/// Whether `chars[index]` starts a word: the first character, one after a separator, an
/// upper-case letter after a lower-case one, or a digit after a letter.
fn is_word_start(chars: &[char], index: usize) -> bool {
    let Some(current) = chars.get(index) else {
        return false;
    };
    let Some(previous) = index.checked_sub(1).and_then(|before| chars.get(before)) else {
        return true;
    };
    !previous.is_alphanumeric()
        || (current.is_uppercase() && !previous.is_uppercase())
        || (current.is_ascii_digit() && !previous.is_ascii_digit())
}

fn same_letter(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

fn is_subsequence(pattern: &[char], chars: &[char]) -> bool {
    let mut remaining = pattern.iter();
    let mut next = remaining.next();
    for c in chars {
        if next.is_some_and(|p| same_letter(*p, *c)) {
            next = remaining.next();
        }
    }
    next.is_none()
}

/// Matches `pattern` against `candidate`. Returns `None` when the characters of `pattern`
/// do not all appear in order, or an upper-case pattern letter cannot land on a word start
/// or a capital letter.
/// An empty pattern matches everything with score 0.
#[must_use]
pub fn fuzzy_match(pattern: &str, candidate: &str) -> Option<FuzzyMatch> {
    let pattern: Vec<char> = pattern.chars().filter(|c| !c.is_whitespace()).collect();
    if pattern.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            positions: Vec::new(),
        });
    }
    let chars: Vec<char> = candidate.chars().collect();
    if pattern.len() > chars.len() {
        return None;
    }
    if !is_subsequence(&pattern, &chars) {
        return None;
    }

    // best[p][c]: the best score of matching pattern[..=p] with pattern[p] on chars[c].
    let (rows, columns) = (pattern.len(), chars.len());
    let mut best = vec![NONE; rows * columns];
    let mut from = vec![usize::MAX; rows * columns];
    let cell = |p: usize, c: usize| p * columns + c;
    for (p, pattern_char) in pattern.iter().enumerate() {
        // The best previous-row score at any earlier column, with its gap cost so far.
        let mut running: Option<(i32, usize)> = None;
        for (c, candidate_char) in chars.iter().enumerate() {
            if p > 0 && c > 0 {
                let previous = best.get(cell(p - 1, c - 1)).copied().unwrap_or(NONE);
                running = match running {
                    Some((score, at)) if score - GAP_PENALTY >= previous => {
                        Some((score - GAP_PENALTY, at))
                    }
                    _ if previous > NONE => Some((previous, c - 1)),
                    other => other.map(|(score, at)| (score - GAP_PENALTY, at)),
                };
            }
            if !same_letter(*pattern_char, *candidate_char) {
                continue;
            }
            let word_start = is_word_start(&chars, c);
            if pattern_char.is_uppercase() && !word_start && !candidate_char.is_uppercase() {
                continue;
            }
            let mut score = MATCH_SCORE;
            if c == 0 {
                score += START_BONUS;
            } else if word_start {
                score += WORD_START_BONUS;
            }
            if pattern_char == candidate_char {
                score += CASE_BONUS;
            }
            let (total, origin) = if p == 0 {
                (
                    score - i32::try_from(c.min(8)).unwrap_or(8) * GAP_PENALTY / 2,
                    usize::MAX,
                )
            } else {
                let Some((previous, at)) = running else {
                    continue;
                };
                let consecutive = if at + 1 == c {
                    CONSECUTIVE_BONUS
                } else {
                    -BREAK_PENALTY
                };
                (previous + score + consecutive, at)
            };
            if let (Some(slot), Some(origin_slot)) =
                (best.get_mut(cell(p, c)), from.get_mut(cell(p, c)))
            {
                *slot = total;
                *origin_slot = origin;
            }
        }
    }

    let last = rows - 1;
    let (mut column, mut score) = (0..columns)
        .filter_map(|c| best.get(cell(last, c)).map(|score| (c, *score)))
        .filter(|(_, score)| *score > NONE)
        .max_by_key(|(c, score)| (*score, std::cmp::Reverse(*c)))?;
    let mut matched = Vec::with_capacity(rows);
    for p in (0..rows).rev() {
        matched.push(column);
        column = from.get(cell(p, column)).copied().unwrap_or(usize::MAX);
    }
    matched.reverse();

    let is_prefix = matched.iter().enumerate().all(|(index, at)| index == *at);
    if is_prefix {
        score += PREFIX_BONUS;
        if rows == columns {
            score += EXACT_BONUS;
        }
    }
    // Among equal matches, shorter candidates are closer to what was typed.
    score -= i32::try_from(columns.min(200)).unwrap_or(200) / 8;
    let offsets: Vec<usize> = candidate.char_indices().map(|(offset, _)| offset).collect();
    let positions = matched
        .into_iter()
        .filter_map(|at| offsets.get(at).copied())
        .collect();
    Some(FuzzyMatch { score, positions })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(pattern: &str, candidate: &str) -> Option<i32> {
        fuzzy_match(pattern, candidate).map(|found| found.score)
    }

    fn marked(pattern: &str, candidate: &str) -> String {
        let Some(found) = fuzzy_match(pattern, candidate) else {
            return "no match".to_owned();
        };
        let mut out = String::new();
        for (offset, c) in candidate.char_indices() {
            if found.positions.contains(&offset) {
                out.push('[');
                out.push(c);
                out.push(']');
            } else {
                out.push(c);
            }
        }
        out
    }

    fn ranks(pattern: &str, ordered: &[&str]) {
        let scores: Vec<i32> = ordered
            .iter()
            .map(|candidate| {
                score(pattern, candidate)
                    .unwrap_or_else(|| panic!("{candidate} did not match {pattern}"))
            })
            .collect();
        for pair in scores.windows(2) {
            assert!(
                pair[0] > pair[1],
                "{pattern}: {ordered:?} scored {scores:?}"
            );
        }
    }

    #[test]
    fn matches_subsequences_case_insensitively() {
        assert_eq!(marked("money", "Money"), "[M][o][n][e][y]");
        assert_eq!(marked("mny", "Money"), "[M]o[n]e[y]");
        assert_eq!(marked("cart", "ShoppingCart"), "Shopping[C][a][r][t]");
        assert_eq!(marked("xyz", "Money"), "no match");
        assert_eq!(marked("moneyy", "Money"), "no match");
        assert_eq!(marked("", "Money"), "Money");
        assert_eq!(marked("é", "café"), "caf[é]");
    }

    #[test]
    fn camel_humps_match_word_starts() {
        assert_eq!(marked("SE", "SearchEverywhere"), "[S]earch[E]verywhere");
        assert_eq!(marked("GTD", "GoToDeclaration"), "[G]o[T]o[D]eclaration");
        assert_eq!(
            marked("SeEv", "SearchEverywhere"),
            "[S][e]arch[E][v]erywhere"
        );
        assert_eq!(marked("fm", "format_money"), "[f]ormat_[m]oney");
        assert_eq!(marked("HM", "HashMap"), "[H]ash[M]ap");
        assert_eq!(
            marked("HM", "Hashmap"),
            "no match",
            "an upper-case letter needs a word start"
        );
        assert_eq!(
            marked("hm", "Hashmap"),
            "[H]ash[m]ap",
            "lower case matches anywhere"
        );
        assert_eq!(
            marked("NPE", "NullPointerException"),
            "[N]ull[P]ointer[E]xception"
        );
        assert_eq!(marked("L2", "Level2Cache"), "[L]evel[2]Cache");
        assert_eq!(
            marked("DEFAULT", "DEFAULT_NAME"),
            "[D][E][F][A][U][L][T]_NAME",
            "constants"
        );
        assert_eq!(marked("DN", "DEFAULT_NAME"), "[D]EFAULT_[N]AME");
    }

    #[test]
    fn prefers_word_starts_over_scattered_letters() {
        assert_eq!(
            marked("cp", "CopyPaste"),
            "[C]opy[P]aste",
            "not the p in Copy"
        );
        assert_eq!(marked("fb", "FooBarBaz"), "[F]oo[B]arBaz");
        assert_eq!(marked("bar", "FooBarBaz"), "Foo[B][a][r]Baz");
    }

    #[test]
    fn ranking() {
        ranks(
            "money",
            &["Money", "MoneyFormatter", "formatMoney", "MonkeyEnvy"],
        );
        ranks("cart", &["Cart", "CartItem", "ShoppingCart", "Chart"]);
        ranks(
            "SE",
            &[
                "SearchEverywhere",
                "SearchEverywhereContributorFactory",
                "BaseSearchEngine",
            ],
        );
        assert_eq!(score("SE", "Sequence"), None, "the E must start a word");
        ranks("prod", &["Product", "ProductRepository", "Reproduced"]);
        ranks("cs", &["CartService", "Access"]);
    }

    #[test]
    fn whitespace_in_the_pattern_is_ignored() {
        assert_eq!(
            marked("search every", "SearchEverywhere"),
            "[S][e][a][r][c][h][E][v][e][r][y]where"
        );
    }

    #[test]
    fn long_inputs_do_not_panic() {
        let long = "ab".repeat(5000);
        assert!(fuzzy_match("abab", &long).is_some());
        assert!(fuzzy_match(&long, "ab").is_none());
        assert!(fuzzy_match("zz", &long).is_none());
    }
}
