//! fzf-style fuzzy matching, used to filter the interface's pickers.
//!
//! Typing `acme` must bring up `tenant acme (prod)` without the user having to know
//! where in the line the letters sit. The scoring follows fzf's: the same letters are
//! worth more at a word boundary, and more still in one uninterrupted run — which is
//! what makes `acme` rank `acme-prod` above `placement`.

/// A successful match.
pub struct Match {
    pub score: i32,
    /// Character positions that matched, ascending and deduplicated — what the
    /// interface highlights.
    pub positions: Vec<usize>,
}

const SCORE_MATCH: i32 = 16;
/// Start of a word: after a space, a `-`, a `/`, a `.`…
const BONUS_BOUNDARY: i32 = SCORE_MATCH / 2;
/// `acmeProd`, `acme2`: a boundary the eye sees without a separator.
const BONUS_CAMEL: i32 = BONUS_BOUNDARY - 1;
const BONUS_CONSECUTIVE: i32 = 4;
/// The query's first character weighs double: it is the one the user aimed at.
const BONUS_FIRST: i32 = 2;
const GAP_START: i32 = -3;
const GAP_EXTENSION: i32 = -1;

/// Matches `query` against `text`, ignoring case.
///
/// Spaces split the query into terms that must *all* match, each anywhere in the text:
/// `acme prod` finds `prod · tenant acme`. An empty query matches everything with a
/// neutral score, which keeps the list in its declared order.
pub fn matches(query: &str, text: &str) -> Option<Match> {
    let hay: Vec<char> = text.chars().collect();
    let low: Vec<char> = hay.iter().copied().map(fold).collect();

    let mut score = 0;
    let mut positions = Vec::new();
    for term in query.split_whitespace() {
        let needle: Vec<char> = term.chars().map(fold).collect();
        let (s, p) = match_term(&needle, &hay, &low)?;
        score += s;
        positions.extend(p);
    }
    positions.sort_unstable();
    positions.dedup();
    Some(Match { score, positions })
}

/// Lowercases a single character, keeping one character for one character: the
/// positions we return index the original text, so a fold that changed the length
/// (`İ` -> `i̇`) would misplace every highlight after it.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Locates one term, then scores the window it occupies.
fn match_term(needle: &[char], hay: &[char], low: &[char]) -> Option<(i32, Vec<usize>)> {
    if needle.is_empty() {
        return Some((0, Vec::new()));
    }

    // Forward pass: the leftmost occurrence of each character, in order. It gives the
    // end of the match — and its failure means the term is not a subsequence at all.
    let mut pidx = 0;
    let mut end = 0;
    for (i, c) in low.iter().enumerate() {
        if *c == needle[pidx] {
            pidx += 1;
            if pidx == needle.len() {
                end = i + 1;
                break;
            }
        }
    }
    if pidx < needle.len() {
        return None;
    }

    // Backward pass from that end: the same letters as far right as possible. What it
    // leaves out cannot contain a better match, and scoring it would only cost time.
    let mut pidx = needle.len();
    let mut start = 0;
    for i in (0..end).rev() {
        if low[i] == needle[pidx - 1] {
            pidx -= 1;
            if pidx == 0 {
                start = i;
                break;
            }
        }
    }

    Some(score_window(needle, hay, low, start, end))
}

/// Greedy walk over `[start, end)`, awarding bonuses and charging gaps.
fn score_window(
    needle: &[char],
    hay: &[char],
    low: &[char],
    start: usize,
    end: usize,
) -> (i32, Vec<usize>) {
    let mut score = 0;
    let mut positions = Vec::with_capacity(needle.len());
    let mut pidx = 0;
    let mut consecutive = 0;
    let mut first_bonus = 0;
    let mut in_gap = false;
    let mut prev = if start == 0 {
        Class::White
    } else {
        class_of(hay[start - 1])
    };

    for i in start..end {
        let cur = class_of(hay[i]);
        if pidx < needle.len() && low[i] == needle[pidx] {
            let mut bonus = bonus_for(prev, cur);
            if consecutive == 0 {
                first_bonus = bonus;
            } else {
                // Inside a run every character inherits the run's opening bonus, unless
                // it crosses a boundary of its own — `acme` in `x-acme` must not lose to
                // `acme` in `xacme` just because the run started one character earlier.
                if bonus == BONUS_BOUNDARY {
                    first_bonus = bonus;
                }
                bonus = bonus.max(first_bonus).max(BONUS_CONSECUTIVE);
            }
            score += SCORE_MATCH
                + if pidx == 0 {
                    bonus * BONUS_FIRST
                } else {
                    bonus
                };
            positions.push(i);
            pidx += 1;
            consecutive += 1;
            in_gap = false;
        } else {
            score += if in_gap { GAP_EXTENSION } else { GAP_START };
            in_gap = true;
            consecutive = 0;
            first_bonus = 0;
        }
        prev = cur;
    }
    (score, positions)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    White,
    /// `-`, `_`, `/`, `.`, `:`… what separates words in a slug, a branch or a URL.
    Delim,
    Lower,
    Upper,
    Digit,
}

fn class_of(c: char) -> Class {
    if c.is_whitespace() {
        Class::White
    } else if c.is_numeric() {
        Class::Digit
    } else if c.is_uppercase() {
        Class::Upper
    } else if c.is_alphabetic() {
        Class::Lower
    } else {
        Class::Delim
    }
}

fn bonus_for(prev: Class, cur: Class) -> i32 {
    match (prev, cur) {
        (_, Class::White) => 0,
        (Class::White | Class::Delim, _) => BONUS_BOUNDARY,
        (Class::Lower, Class::Upper) => BONUS_CAMEL,
        (p, Class::Digit) if p != Class::Digit => BONUS_CAMEL,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(query: &str, text: &str) -> Option<i32> {
        matches(query, text).map(|m| m.score)
    }

    #[test]
    fn matches_a_subsequence_anywhere() {
        assert!(score("acme", "tenant acme (prod)").is_some());
        assert!(score("tnt", "tenant").is_some());
        assert!(score("zz", "tenant acme").is_none());
        // Order matters: the letters must appear in the typed order.
        assert!(score("emca", "acme").is_none());
    }

    #[test]
    fn ignores_case_and_accents_length() {
        assert!(score("SOCI", "société acme").is_some());
        let m = matches("soci", "Société").unwrap();
        assert_eq!(m.positions, vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_run_beats_scattered_letters() {
        assert!(score("abc", "abcdef") > score("abc", "axbxcx"));
    }

    #[test]
    fn a_word_start_beats_the_middle_of_a_word() {
        assert!(score("acme", "prod-acme") > score("acme", "prodacme"));
        assert!(score("bar", "foo bar") > score("bar", "foobar"));
    }

    /// Two equally good matches score the same on purpose: what separates
    /// `acme-prod` from `prod-acme` is where the match starts, and that tie is broken
    /// when the list is sorted, not here.
    #[test]
    fn an_equally_good_match_scores_the_same() {
        assert_eq!(score("acme", "acme-prod"), score("acme", "prod-acme"));
        assert_eq!(matches("acme", "acme-prod").unwrap().positions[0], 0);
        assert_eq!(matches("acme", "prod-acme").unwrap().positions[0], 5);
    }

    #[test]
    fn every_space_separated_term_must_match() {
        assert!(score("acme prod", "prod · tenant acme").is_some());
        assert!(score("acme staging", "prod · tenant acme").is_none());
    }

    #[test]
    fn positions_point_at_the_matched_characters() {
        let m = matches("ac", "tenant acme").unwrap();
        assert_eq!(m.positions, vec![7, 8]);
        // Two terms: the union of both, sorted.
        let m = matches("me te", "tenant acme").unwrap();
        assert_eq!(m.positions, vec![0, 1, 9, 10]);
    }

    #[test]
    fn an_empty_query_matches_everything() {
        let m = matches("   ", "anything").unwrap();
        assert_eq!(m.score, 0);
        assert!(m.positions.is_empty());
    }

    #[test]
    fn camel_and_digits_are_boundaries() {
        assert!(score("prod", "acmeProd") > score("prod", "acmeprod"));
        assert!(score("2", "acme2") > score("2", "12"));
    }
}
