//! Search over entry and folder names. It never looks inside an entry:
//! that would take the key, and names are all paw may remember.
//!
//! Letter case and accents are ignored (`zurich` finds `Zürich`). Every
//! word of the query must appear somewhere in the entry's path. Results are
//! ordered by how well the words fit, best first:
//!
//! 1. the word is the entry's whole name
//! 2. the entry's name starts with it
//! 3. a word inside the entry's name starts with it
//! 4. a folder name, or a word inside one, starts with it
//! 5. it appears elsewhere in the entry's name
//! 6. it appears elsewhere in a folder name
//!
//! With several words the fits are added up. Ties go to the shorter path,
//! then alphabetically.

use unicode_normalization::char::{decompose_canonical, is_combining_mark};

use super::tree::EntryRef;

/// A stretch of text to highlight, in UTF-16 code units, which is how
/// `NSRange` and `String.Index(utf16Offset:in:)` count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct HighlightRange {
    pub start: u32,
    pub len: u32,
}

/// One search result, with what to highlight in each line of its row.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SearchHit {
    pub entry: EntryRef,
    /// Ranges in `entry.name`.
    pub name_ranges: Vec<HighlightRange>,
    /// Ranges in `entry.folder`.
    pub folder_ranges: Vec<HighlightRange>,
}

/// Text with case and accents removed, remembering which characters of the
/// original each folded character stands for.
struct Folded {
    chars: Vec<char>,
    /// Index of the original character each folded one came from.
    origin: Vec<usize>,
    /// One past the last original character it covers: a letter takes the
    /// accents written after it along, so a highlight never splits them.
    end: Vec<usize>,
}

fn fold(text: &str) -> Folded {
    let mut folded = Folded { chars: Vec::new(), origin: Vec::new(), end: Vec::new() };
    for (index, c) in text.chars().enumerate() {
        // "ü" becomes "u" plus a combining mark; the mark is dropped.
        decompose_canonical(c, |part| {
            if is_combining_mark(part) {
                if let Some(end) = folded.end.last_mut() {
                    *end = index + 1;
                }
            } else {
                for lower in part.to_lowercase() {
                    folded.chars.push(lower);
                    folded.origin.push(index);
                    folded.end.push(index + 1);
                }
            }
        });
    }
    folded
}

/// Where one query word fits best in one path: `(fit, position)`, lower fit
/// is better. `None` if it does not appear.
fn best_fit(path: &Folded, name_start: usize, term: &[char]) -> Option<(u32, usize)> {
    let name_len = path.chars.len() - name_start;
    (0..path.chars.len().checked_sub(term.len())? + 1)
        .filter(|&at| path.chars[at..at + term.len()] == *term)
        .map(|at| {
            let word_start = at == 0 || !path.chars[at - 1].is_alphanumeric();
            let fit = match (at >= name_start, at == name_start, word_start) {
                (true, true, _) if term.len() == name_len => 0,
                (true, true, _) => 1,
                (true, false, true) => 2,
                (false, _, true) => 3,
                (true, false, false) => 4,
                (false, _, false) => 5,
            };
            (fit, at)
        })
        .min()
}

/// Sorts and joins ranges of character indices that touch or overlap.
fn merged(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        match out.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => out.push((start, end)),
        }
    }
    out
}

/// Turns character ranges of `text` into UTF-16 ranges.
fn utf16_ranges(text: &str, ranges: Vec<(usize, usize)>) -> Vec<HighlightRange> {
    let mut offsets = Vec::with_capacity(text.len() + 1);
    let mut units = 0u32;
    for c in text.chars() {
        offsets.push(units);
        units += c.len_utf16() as u32;
    }
    offsets.push(units);
    merged(ranges)
        .into_iter()
        .map(|(start, end)| HighlightRange {
            start: offsets[start],
            len: offsets[end] - offsets[start],
        })
        .collect()
}

/// Finds the entries whose path contains every word of `query`, best first.
/// At most `limit` are returned. An empty query finds nothing.
pub fn search(entries: &[EntryRef], query: &str, limit: u32) -> Vec<SearchHit> {
    let terms: Vec<Vec<char>> = query
        .split_whitespace()
        .map(|term| fold(term).chars)
        .filter(|term| !term.is_empty())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }

    // (fit, path length, entry, character ranges of the path to highlight)
    type Scored<'a> = (u32, usize, &'a EntryRef, Vec<(usize, usize)>);
    let mut hits: Vec<Scored<'_>> = Vec::new();
    for entry in entries {
        let path = fold(&entry.path);
        let folder_chars = entry.folder.chars().count();
        // In characters of the path: the name follows the folder and a `/`.
        let name_origin = if entry.folder.is_empty() { 0 } else { folder_chars + 1 };
        let name_start = path.origin.partition_point(|&origin| origin < name_origin);

        let mut score = 0;
        let mut ranges = Vec::with_capacity(terms.len());
        let all = terms.iter().all(|term| {
            let Some((fit, at)) = best_fit(&path, name_start, term) else {
                return false;
            };
            score += fit;
            ranges.push((path.origin[at], path.end[at + term.len() - 1]));
            true
        });
        if all {
            hits.push((score, entry.path.chars().count(), entry, ranges));
        }
    }
    hits.sort_by(|a, b| {
        (a.0, a.1).cmp(&(b.0, b.1)).then_with(|| {
            (a.2.path.to_lowercase(), &a.2.path).cmp(&(b.2.path.to_lowercase(), &b.2.path))
        })
    });
    hits.truncate(limit as usize);

    hits.into_iter()
        .map(|(_, _, entry, ranges)| {
            let folder_chars = entry.folder.chars().count();
            let name_origin = if entry.folder.is_empty() { 0 } else { folder_chars + 1 };
            // A word like `web/git` straddles the `/`: split it in two.
            let in_folder = ranges
                .iter()
                .filter(|(start, _)| *start < folder_chars)
                .map(|&(start, end)| (start, end.min(folder_chars)))
                .collect();
            let in_name = ranges
                .iter()
                .filter(|(_, end)| *end > name_origin)
                .map(|&(start, end)| (start.max(name_origin) - name_origin, end - name_origin))
                .collect();
            SearchHit {
                entry: entry.clone(),
                name_ranges: utf16_ranges(&entry.name, in_name),
                folder_ranges: utf16_ranges(&entry.folder, in_folder),
            }
        })
        .collect()
}
