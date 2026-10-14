//! Filename parsing for movies, episodes, and audio.
//!
//! Covers the common conventions:
//! * `The Matrix (1999).mkv`
//! * `The Matrix 1999 1080p BluRay x264.mkv`
//! * `Show/Season 01/S01E02 - Pilot.mkv`
//! * `01x02 - Title.mkv`

use serde::{Deserialize, Serialize};

/// Structured hints extracted from a filename.
///
/// Deliberately conservative: fields stay `None` rather than guessing, so a
/// metadata provider can fill in the gaps without being contradicted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParsedName {
    /// Primary display title, cleaned of year/tags.
    pub title: String,
    /// Four-digit release year, if present.
    pub year: Option<u32>,
    /// Season number for episodes.
    pub season: Option<u32>,
    /// Episode number for episodes.
    pub episode: Option<u32>,
    /// Episode title when the name uses `SxxEyy - Title` form.
    pub episode_title: Option<String>,
    /// Show name, taken from the text before the episode marker.
    ///
    /// This is the fallback for libraries that keep every episode flat in one
    /// folder; when episodes live in `Show/Season 01/`, the folder layout is a
    /// better source and the scanner prefers it.
    pub show: Option<String>,
}

/// Parses filenames into [`ParsedName`].
#[derive(Debug, Clone, Copy, Default)]
pub struct NameParser;

/// Tokens that mark the end of a title when no year is present.
const QUALITY_TOKENS: &[&str] = &[
    "1080p", "720p", "480p", "2160p", "4k", "x264", "x265", "h264", "h265", "hevc", "avc",
    "bluray", "blu-ray", "bdrip", "brrip", "webrip", "web-dl", "webdl", "hdtv", "dvdrip",
    "dvdscr", "remux", "hdr", "hdr10", "dolbyvision", "dv", "aac", "ac3", "eac3", "dts",
    "dts-hd", "truehd", "atmos", "flac", "mp3", "ogg", "opus", "xvid", "divx", "mkv", "mp4",
    "avi", "imax", "extended", "unrated", "remastered", "proper", "repack", "internal",
];

impl NameParser {
    pub fn new() -> Self {
        Self
    }

    /// Parse a path's filename (extension ignored).
    pub fn parse<P: AsRef<std::path::Path>>(&self, path: P) -> ParsedName {
        let file_name = path
            .as_ref()
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        self.parse_name(&file_name)
    }

    /// Parse a bare filename stem (no directory, no extension).
    pub fn parse_name(&self, name: &str) -> ParsedName {
        let stem = strip_extension(name);

        // Episode patterns take priority: "S01E02", "1x02", "s1e2e3".
        if let Some(parsed) = self.try_episode(stem) {
            return parsed;
        }

        self.try_movie(stem)
    }

    /// Recognize `SxxEyy`, `xxEyy`, and `xxx` episode forms.
    fn try_episode(&self, stem: &str) -> Option<ParsedName> {
        let lower = stem.to_ascii_lowercase();

        // Multi-episode "S01E02E03": collapse to the first, note the run.
        let (season, episode, rest) = if let Some((s, e, rest)) = split_season_episode(&lower, stem)
        {
            (s, e, rest)
        } else {
            let (s, e) = split_x_y(&lower)?;
            (Some(s), Some(e), "")
        };

        // Title is whatever follows " - " or " " after the marker.
        let title = extract_trailing_title(rest)
            .filter(|t| !t.is_empty())
            .or_else(|| {
                // "Show - 1x02 Title" leaves the show name before the marker.
                stem.split_once(" - ").map(|(show, _)| show.trim().to_owned())
            });

        // "Show - S01E02 - Title" names the show before the marker. Take it
        // from there so a flat library still groups episodes into a series.
        let show = stem
            .split_once(" - ")
            .map(|(head, _)| head)
            .or_else(|| {
                // "Show.S01E02.Title" — everything before the marker.
                find_episode_marker(&lower).map(|at| &stem[..at])
            })
            .map(|head| head.trim_matches(|c: char| c == '-' || c == '.' || c == '_' || c.is_whitespace()))
            .filter(|head| !head.is_empty() && !is_marker_only(head))
            .map(|head| tidy(head));

        Some(ParsedName {
            title: title.clone().unwrap_or_else(|| stem.to_owned()),
            year: None,
            season,
            episode,
            episode_title: title.clone(),
            show,
        })
    }

    /// Recognize `Title (Year)` and `Title Year Tags`.
    fn try_movie(&self, stem: &str) -> ParsedName {
        // Normalize separators FIRST. Release-style names separate words with
        // dots or underscores ("The.Matrix.1999.1080p.BluRay.x264-GROUP"), so
        // year and tag detection only work once those become spaces.
        let mut title = tidy(stem);
        let mut year = None;

        // "Title (1999)" or "Title [1999]".
        if let Some((head, tail)) = split_paren_year(&title) {
            if let Some(y) = tail {
                title = head.trim_end().to_owned();
                year = Some(y);
            }
        }

        // "Title 1999 1080p ..." — take the first standalone 4-digit token.
        if year.is_none() {
            if let Some((head, y)) = split_bare_year(&title) {
                year = Some(y);
                title = head.trim_end().to_owned();
            }
        }

        // Drop trailing bracketed quality groups, then trailing release tags.
        title = strip_release_tags(&title);
        title = tidy(&title);

        ParsedName { title, year, season: None, episode: None, episode_title: None, show: None }
    }
}

impl ParsedName {
    pub fn show_name(&self) -> Option<String> {
        self.show.clone().filter(|s| !s.is_empty())
    }

    pub fn season_number(&self) -> Option<u32> {
        self.season
    }

    pub fn episode_number(&self) -> Option<u32> {
        self.episode
    }

    /// Sort key: leading article ignored, so "The Matrix" sorts under "M".
    pub fn sort_title(&self) -> String {
        let t = self.title.trim_start_matches(|c: char| !c.is_alphanumeric());
        for article in ["The ", "A ", "An "] {
            if let Some(rest) = t.strip_prefix(article) {
                return rest.to_ascii_lowercase();
            }
        }
        t.to_ascii_lowercase()
    }
}

/// Remove a trailing media extension.
fn strip_extension(name: &str) -> &str {
    for ext in [".mkv", ".mp4", ".avi", ".m4v", ".mov", ".ts", ".webm", ".mpg", ".mpeg"] {
        if let Some(stem) = name.strip_suffix(ext) {
            return stem;
        }
        if let Some(stem) = name.strip_suffix(ext.to_ascii_uppercase().as_str()) {
            return stem;
        }
    }
    // Audio extensions, plus the fallback of dropping any final ".ext".
    for ext in [".flac", ".mp3", ".ogg", ".opus", ".m4a", ".wav", ".wma", ".aac", ".aiff"] {
        if let Some(stem) = name.strip_suffix(ext) {
            return stem;
        }
    }
    if let Some(idx) = name.rfind('.') {
        if idx > 0 && name[idx + 1..].len() <= 4 && name[idx + 1..].chars().all(|c| c.is_alphanumeric())
        {
            return &name[..idx];
        }
    }
    name
}

/// Match `s01e02`, capturing season, episode, and the trailing remainder.
fn split_season_episode<'a>(
    lower: &str,
    original: &'a str,
) -> Option<(Option<u32>, Option<u32>, &'a str)> {
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b's' {
            let mut j = i + 1;
            let season_start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j == season_start || j >= bytes.len() || bytes[j] != b'e' {
                i += 1;
                continue;
            }
            let season = lower[season_start..j].parse::<u32>().ok();
            let mut k = j + 1;
            let ep_start = k;
            while k < bytes.len() && bytes[k].is_ascii_digit() {
                k += 1;
            }
            if k == ep_start {
                i += 1;
                continue;
            }
            let episode = lower[ep_start..k].parse::<u32>().ok();
            // Remainder in the original casing, mapped to the same byte offset.
            let rest = &original[k..];
            return Some((season, episode, rest));
        }
        i += 1;
    }
    None
}

/// Match `1x02` with no `s`/`e` letters.
fn split_x_y(lower: &str) -> Option<(u32, u32)> {
    let mut parts = lower.splitn(2, 'x');
    let left = parts.next()?;
    let right = parts.next()?;
    if left.is_empty() || right.is_empty() || !left.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Only accept if the digits appear at the start (after an optional dash).
    let left = left.trim_start_matches(|c| c == '-' || c == ' ');
    let right_digits: String = right.chars().take_while(|c| c.is_ascii_digit()).collect();
    if left.is_empty() || right_digits.is_empty() {
        return None;
    }
    Some((left.parse().ok()?, right_digits.parse().ok()?))
}

/// Byte offset of an `s01e02` / `1x02` marker in a lowercased filename.
///
/// The marker must not start in the middle of a word, so `Sopranos2x04` is
/// rejected in favour of `The.Sopranos.2x04`.
fn find_episode_marker(lower: &str) -> Option<usize> {
    let bytes = lower.as_bytes();

    let is_word = |i: usize| i > 0 && (bytes[i - 1] as char).is_alphanumeric();
    let digit_run = |from: usize| -> usize {
        bytes[from..].iter().take_while(|b| b.is_ascii_digit()).count()
    };

    for i in 0..bytes.len() {
        if is_word(i) {
            continue;
        }
        match bytes[i] {
            // s01e02, optionally followed by more episodes (s01e02e03).
            b's' => {
                let season_len = digit_run(i + 1);
                if season_len == 0 {
                    continue;
                }
                let e = i + 1 + season_len;
                if e < bytes.len() && bytes[e] == b'e' && digit_run(e + 1) > 0 {
                    return Some(i);
                }
            }
            // 1x02
            _ => {
                let season_len = digit_run(i);
                if season_len == 0 {
                    continue;
                }
                let x = i + season_len;
                if x < bytes.len() && bytes[x] == b'x' && digit_run(x + 1) > 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Whether a fragment is nothing but an episode marker (`01x02`, `S01E02`).
///
/// Such a fragment is a filename, not a show name: `01x02 - Title.mkv` has no
/// series to attach to.
fn is_marker_only(fragment: &str) -> bool {
    find_episode_marker(&fragment.to_ascii_lowercase()) == Some(0)
}

/// Whether a directory name is a season folder (`Season 01`, `Specials`).
pub fn is_season_dir(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    if lower == "specials" || lower == "special" {
        return true;
    }
    let rest = lower.strip_prefix("season").map(str::trim_start).unwrap_or("");
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    !digits.is_empty() && rest[digits.len()..].trim().is_empty()
}

/// Split `Title (1999)` / `Title [1999]`, validating the year range.
fn split_paren_year(stem: &str) -> Option<(&str, Option<u32>)> {
    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
        if let Some(start) = stem.rfind(open) {
            let end = stem[start + 1..].find(close)? + start + 1;
            let inner = &stem[start + 1..end];
            if let Some(year) = parse_year(inner) {
                return Some((&stem[..start], Some(year)));
            }
        }
    }
    None
}

/// Find a bare 4-digit year that stands alone as a word, e.g. `Title 1999 1080p`.
///
/// The token must be exactly four digits (so `19999` and `1080` are rejected),
/// must parse as a plausible year, and must be followed by a word boundary so
/// that `Movie 20163D` is not read as a year.
fn split_bare_year(text: &str) -> Option<(&str, u32)> {
    for (space, _) in text.match_indices(' ') {
        let after = &text[space + 1..];
        let token: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();

        if token.len() != 4 {
            continue;
        }
        let Some(year) = parse_year(&token) else { continue };

        // The year must be a whole word: nothing but a space or bracket may
        // follow it.
        let rest = &after[token.len()..];
        let bounded =
            rest.is_empty() || rest.starts_with(' ') || rest.starts_with(['(', '[', '{']);
        if bounded {
            return Some((&text[..space], year));
        }
    }
    None
}

/// Validate a plausible movie/TV year.
fn parse_year(raw: &str) -> Option<u32> {
    let y: u32 = raw.trim().parse().ok()?;
    (1888..=2100).contains(&y).then_some(y)
}

/// Strip trailing release tags, bracketed groups, and dotted separators.
fn strip_release_tags(title: &str) -> String {
    let mut t = title.trim().to_owned();

    // Remove trailing bracketed groups that encode quality info, e.g.
    // "Arrival 2016 [1080p] [Bluray]". Groups whose content looks like a real
    // subtitle, such as "(Director's Cut)", are deliberately preserved.
    loop {
        let Some(trimmed) = t.trim_end().strip_suffix([']', '}', ')']) else { break };
        let Some(open) = trimmed.rfind(['[', '{', '(']) else { break };

        // Nested brackets mean this is not a simple suffix group.
        let inner = &trimmed[open + 1..];
        if inner.contains(['[', ']', '{', '}', '(', ')']) {
            break;
        }

        let normalized_inner = inner.trim().to_ascii_lowercase();
        let looks_like_tag = inner.contains(|c: char| c.is_ascii_digit())
            || QUALITY_TOKENS.contains(&normalized_inner.as_str());

        if looks_like_tag {
            t = trimmed[..open].trim_end().to_owned();
        } else {
            break;
        }
    }

    // Remove trailing whitespace-separated release tokens.
    //
    // Only tokens that actually look technical are removed. An earlier version
    // also dropped any short token containing a digit, which ate real titles:
    // "Blade Runner 2049" became "Blade Runner" and "Se7en" became "S".
    loop {
        let trimmed = t.trim_end();
        let Some(last) = trimmed.rsplit(' ').next() else { break };
        if last.is_empty() {
            break;
        }

        let normalized = last
            .trim_matches(|c: char| c == '.' || c == '-' || c == '_')
            .to_ascii_lowercase();
        // "x264-GROUP" is a codec tag; its first segment is what identifies it.
        let head = normalized.split(['-', '.']).next().unwrap_or("");
        let is_tag = QUALITY_TOKENS.contains(&normalized.as_str())
            || QUALITY_TOKENS.contains(&head)
            || is_resolution(last);

        if is_tag {
            t = trimmed[..trimmed.len() - last.len()].trim_end().to_owned();
        } else {
            break;
        }
    }

    t
}

/// Whether a token is a resolution such as `1080p`, `720i` or `2160p`.
///
/// A trailing letter is required on purpose: a bare `2049` is a year, and
/// belongs to the title.
fn is_resolution(token: &str) -> bool {
    let digits = token.len() - token.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if !(3..=4).contains(&digits) {
        return false;
    }
    let suffix = &token[digits..];
    !suffix.is_empty() && suffix.len() <= 2 && suffix.chars().all(|c| c.is_ascii_alphabetic())
}

/// Collapse separators and trim punctuation left behind by tag stripping.
fn tidy(title: &str) -> String {
    let mut t: String = title
        .replace('.', " ")
        .replace('_', " ")
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();

    // Collapse runs of whitespace.
    let mut out = String::with_capacity(t.len());
    let mut prev_space = true;
    for c in t.chars() {
        if c == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    t = out.trim().to_owned();

    // Trim dangling separators from stripped tags: "Movie -", "Movie --".
    while t.ends_with('-') || t.ends_with('–') || t.ends_with('—') {
        t.pop();
        t = t.trim_end().to_owned();
    }
    t
}

/// Extract the title that follows a ` - ` or `. ` separator.
fn extract_trailing_title(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    let rest = rest.trim_start_matches(['-', '–', '—', '.', '_', ' ']);
    let candidate = rest.trim();
    if candidate.is_empty() {
        return None;
    }
    let cleaned = tidy(candidate);
    (!cleaned.is_empty()).then_some(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(name: &str) -> ParsedName {
        NameParser::new().parse_name(name)
    }

    #[test]
    fn movie_with_parenthesized_year() {
        let p = parse("The Matrix (1999).mkv");
        assert_eq!(p.title, "The Matrix");
        assert_eq!(p.year, Some(1999));
    }

    #[test]
    fn movie_with_release_tags() {
        let p = parse("The.Matrix.1999.1080p.BluRay.x264-GROUP");
        assert_eq!(p.title, "The Matrix");
        assert_eq!(p.year, Some(1999));
    }

    #[test]
    fn movie_with_bare_year_and_brackets() {
        let p = parse("Arrival 2016 [1080p]");
        assert_eq!(p.title, "Arrival");
        assert_eq!(p.year, Some(2016));
    }

    #[test]
    fn episode_season_episode_form() {
        let p = parse("Show - S01E02 - The Title.mkv");
        assert_eq!(p.season, Some(1));
        assert_eq!(p.episode, Some(2));
        assert_eq!(p.episode_title.as_deref(), Some("The Title"));
    }

    #[test]
    fn episode_x_y_form() {
        let p = parse("01x02 - The Title.mkv");
        assert_eq!(p.season, Some(1));
        assert_eq!(p.episode, Some(2));
    }

    #[test]
    fn show_name_comes_from_before_the_marker() {
        let p = parse("Twin Peaks - S01E01 - Pilot.mkv");
        assert_eq!(p.show.as_deref(), Some("Twin Peaks"));
        assert_eq!(p.episode_title.as_deref(), Some("Pilot"));

        // Dotted release naming works too.
        let dotted = parse("The.Office.S02E05.mkv");
        assert_eq!(dotted.show.as_deref(), Some("The Office"));
    }

    #[test]
    fn a_bare_marker_is_not_a_show_name() {
        // "01x02 - Title" has no series to belong to.
        assert_eq!(parse("01x02 - The Title.mkv").show, None);
        assert_eq!(parse("S01E02.mkv").show, None);
        // Movies never produce one.
        assert_eq!(parse("The Matrix (1999).mkv").show, None);
    }

    #[test]
    fn season_directories_are_recognized() {
        assert!(is_season_dir("Season 01"));
        assert!(is_season_dir("season 12"));
        assert!(is_season_dir("Season2"));
        assert!(is_season_dir("Specials"));
        assert!(!is_season_dir("Season One Finale"));
        assert!(!is_season_dir("Twin Peaks"));
        assert!(!is_season_dir("Season"));
    }

    #[test]
    fn multi_episode_collapses_to_first() {
        let p = parse("Show S01E02E03.mkv");
        assert_eq!(p.season, Some(1));
        assert_eq!(p.episode, Some(2));
    }

    #[test]
    fn digits_that_belong_to_the_title_survive() {
        // The trailing number is part of the name, not a release tag.
        assert_eq!(parse("Blade Runner 2049 (2017).mkv").title, "Blade Runner 2049");
        assert_eq!(parse("Film 01 (2001).mkv").title, "Film 01");
        assert_eq!(parse("Se7en (1995).mkv").title, "Se7en");
        assert_eq!(parse("Apollo 13 (1995).mkv").title, "Apollo 13");
        // ...while technical tokens still go.
        assert_eq!(parse("Se7en 1995 1080p.mkv").title, "Se7en");
        assert_eq!(parse("Amour 2012 2160p HDR.mkv").title, "Amour");
    }

    #[test]
    fn year_out_of_range_is_not_taken() {
        let p = parse("Movie 999 Title");
        assert_ne!(p.year, Some(999));
    }

    #[test]
    fn sort_title_ignores_leading_article() {
        let p = parse("The Matrix (1999).mkv");
        assert_eq!(p.sort_title(), "matrix");
    }

    #[test]
    fn audio_track_parses() {
        let p = parse("01 - Artist - Album - 01 Track.flac");
        assert!(p.title.contains("Track"));
    }
}