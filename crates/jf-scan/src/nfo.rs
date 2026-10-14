//! Local NFO sidecar metadata (Kodi-style).
//!
//! Only the fields this server persists are parsed; unknown elements are
//! ignored so real-world files load without error.

use crate::naming::NameParser;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Metadata extracted from an NFO file.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct NfoMetadata {
    pub title: Option<String>,
    pub original_title: Option<String>,
    pub sort_title: Option<String>,
    pub year: Option<i32>,
    pub plot: Option<String>,
    pub outline: Option<String>,
    pub tagline: Option<String>,
    pub rating: Option<f32>,
    pub critic_rating: Option<f32>,
    /// Runtime in 100ns ticks, converted from NFO's whole minutes.
    pub runtime_ticks: Option<i64>,
    pub premiered: Option<String>,
    pub status: Option<String>,
    pub mpaa: Option<String>,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub studios: Vec<String>,
    pub actors: Vec<NfoPerson>,
    pub directors: Vec<NfoPerson>,
    pub writers: Vec<NfoPerson>,
    /// External ids keyed by provider: `imdb`, `tmdb`, `tvdb`.
    pub external_ids: BTreeMap<String, String>,
    /// Image references, in document order.
    pub images: Vec<NfoImage>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct NfoPerson {
    pub name: String,
    pub role: Option<String>,
    pub thumb: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NfoImage {
    pub path: String,
    /// `poster`, `fanart`, `thumb`, `banner`, or `None` for the primary.
    pub kind: Option<String>,
}

impl NfoMetadata {
    /// True when the file carried no usable fields.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.plot.is_none()
            && self.genres.is_empty()
            && self.external_ids.is_empty()
            && self.images.is_empty()
    }

    /// Best available description.
    pub fn best_plot(&self) -> Option<&str> {
        self.plot.as_deref().or(self.outline.as_deref())
    }
}

#[derive(Debug, Error)]
pub enum NfoError {
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed XML: {0}")]
    Xml(String),
}

/// Find the NFO that applies to a media file.
///
/// Preference order: `<file>.nfo`, then `movie.nfo` / `tvshow.nfo` in the
/// same directory, since those apply to a whole folder.
pub fn find_nfo(media_path: &Path) -> Option<PathBuf> {
    let sidecar = media_path.with_extension("nfo");
    if sidecar.is_file() {
        return Some(sidecar);
    }

    let dir = media_path.parent()?;
    for name in ["movie.nfo", "tvshow.nfo"] {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Parse an NFO file from disk.
pub fn parse_nfo(path: &Path) -> Result<NfoMetadata, NfoError> {
    let xml = std::fs::read_to_string(path).map_err(|source| NfoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_nfo_str(&xml)
}

/// Which repeated person element is currently open.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PersonKind {
    Actor,
    Director,
    Writer,
}

/// Parse NFO XML from a string.
pub fn parse_nfo_str(xml: &str) -> Result<NfoMetadata, NfoError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut meta = NfoMetadata::default();
    let mut person_kind: Option<PersonKind> = None;
    let mut person = NfoPerson::default();

    // The element whose text node we are currently reading. Text and element
    // name arrive in separate events, so this bridges them.
    let mut current: Option<String> = None;
    let mut unique_id_type: Option<String> = None;
    // `fanart` wraps `<thumb>`; remember so its images are tagged correctly.
    let mut in_fanart = false;
    // quick-xml does not reject unclosed elements at EOF, but a truncated NFO
    // silently losing half its metadata is worse than a parse error.
    let mut depth: usize = 0;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                depth += 1;
                let name = e.local_name().as_ref().to_ascii_lowercase();
                match name.as_str() {
                    // Entering a person container starts a fresh record. This
                    // MUST be done per container and not for every nested
                    // element: resetting on `<name>` or `<role>` would discard
                    // the fields already collected in an
                    // `<actor><name/><role/></actor>` block.
                    "actor" => {
                        person_kind = Some(PersonKind::Actor);
                        person = NfoPerson::default();
                    }
                    "director" => {
                        person_kind = Some(PersonKind::Director);
                        person = NfoPerson::default();
                    }
                    "writer" | "credits" => {
                        person_kind = Some(PersonKind::Writer);
                        person = NfoPerson::default();
                    }
                    "fanart" => in_fanart = true,
                    "uniqueid" => {
                        unique_id_type = attribute(&e, b"type").map(|s| s.to_ascii_lowercase());
                    }
                    _ => {}
                }
                current = Some(name);
            }
            Ok(Event::Empty(e)) => {
                let name = e.local_name().as_ref().to_ascii_lowercase();
                if name == "uniqueid" {
                    if let Some(t) = attribute(&e, b"type") {
                        // An empty uniqueid carries no value; ignore it.
                        let _ = t;
                    }
                }
            }
            Ok(Event::End(e)) => {
                depth = depth.saturating_sub(1);
                let name = e.local_name().as_ref().to_ascii_lowercase();
                match name.as_str() {
                    "actor" | "director" | "writer" | "credits" => {
                        if let Some(kind) = person_kind.take() {
                            if !person.name.trim().is_empty() {
                                let p = person.clone();
                                match kind {
                                    PersonKind::Actor => meta.actors.push(p),
                                    PersonKind::Director => meta.directors.push(p),
                                    PersonKind::Writer => meta.writers.push(p),
                                }
                            }
                        }
                        person = NfoPerson::default();
                    }
                    "fanart" => in_fanart = false,
                    "uniqueid" => unique_id_type = None,
                    _ => {}
                }
                current = None;
            }
            Ok(Event::Text(t)) => {
                let value = t.into_inner().trim().to_owned();
                if value.is_empty() {
                    continue;
                }
                let Some(element) = current.clone() else { continue };

                if person_kind.is_some() {
                    match element.as_str() {
                        "name" => person.name = value,
                        "role" => person.role = Some(value),
                        "thumb" => person.thumb = Some(value),
                        _ => {}
                    }
                    continue;
                }

                match element.as_str() {
                    "title" => meta.title = Some(value),
                    "originaltitle" => meta.original_title = Some(value),
                    "sorttitle" => meta.sort_title = Some(value),
                    "year" => meta.year = value.parse().ok(),
                    "plot" => meta.plot = Some(value),
                    "outline" => meta.outline = Some(value),
                    "tagline" => meta.tagline = Some(value),
                    "rating" => meta.rating = value.parse().ok(),
                    "criticrating" => meta.critic_rating = value.parse().ok(),
                    "status" => meta.status = Some(value),
                    "premiered" => meta.premiered = Some(value),
                    "mpaa" | "certification" => meta.mpaa = Some(value),
                    "runtime" => {
                        if let Ok(mins) = value.trim().parse::<i64>() {
                            meta.runtime_ticks = Some(mins * jf_core::TICKS_PER_MINUTE);
                        }
                    }
                    "genre" => meta.genres.push(value),
                    "tag" => meta.tags.push(value),
                    "studio" => meta.studios.push(value),
                    "uniqueid" => {
                        if let Some(provider) = unique_id_type.take() {
                            meta.external_ids.insert(provider, value);
                        }
                    }
                    "thumb" => {
                        let aspect = e_aspect_hint(&value);
                        meta.images.push(NfoImage {
                            kind: if in_fanart {
                                Some("backdrop".into())
                            } else {
                                Some(aspect)
                            },
                            path: value,
                        });
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => {
                if depth > 0 {
                    return Err(NfoError::Xml(format!(
                        "unclosed element at end of file ({depth} still open)"
                    )));
                }
                break;
            }
            Err(e) => return Err(NfoError::Xml(e.to_string())),
            _ => {}
        }
    }

    Ok(meta)
}

/// Read an attribute as an owned string.
fn attribute(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == std::str::from_utf8(key).unwrap_or_default())
        .and_then(|a| a.unescape_value().ok())
        .map(|v| v.to_string())
}

/// Kodi records the image role in the filename suffix (e.g. `-poster.jpg`).
fn e_aspect_hint(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    for hint in ["poster", "fanart", "banner", "thumb", "clearart", "discart", "landscape"] {
        if lower.contains(&format!("-{hint}")) || lower.contains(&format!("_{hint}")) {
            return hint.to_owned();
        }
    }
    "primary".to_owned()
}

/// Fallback hints from the filename when no NFO exists.
pub fn hints_from_path(path: &Path) -> crate::naming::ParsedName {
    NameParser::new().parse(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<movie>
  <title>Blade Runner</title>
  <originaltitle>Blade Runner</originaltitle>
  <sorttitle>Blade Runner</sorttitle>
  <year>1982</year>
  <runtime>117</runtime>
  <plot>In a dystopian future...</plot>
  <outline>Short summary</outline>
  <tagline>Man has made his match...</tagline>
  <rating>8.1</rating>
  <criticrating>89</criticrating>
  <mpaa>R</mpaa>
  <premiered>1982-06-25</premiered>
  <status>Released</status>
  <genre>Sci-Fi</genre>
  <genre>Thriller</genre>
  <tag>cyberpunk</tag>
  <studio>Warner Bros.</studio>
  <uniqueid type="imdb" default="true">0083658</uniqueid>
  <uniqueid type="tmdb">78</uniqueid>
  <actor><name>Harrison Ford</name><role>Deckard</role></actor>
  <thumb>poster.jpg</thumb>
  <fanart><thumb>fanart.jpg</thumb></fanart>
</movie>"#;

    #[test]
    fn parses_full_movie_nfo() {
        let meta = parse_nfo_str(SAMPLE).expect("valid xml");
        assert_eq!(meta.title.as_deref(), Some("Blade Runner"));
        assert_eq!(meta.year, Some(1982));
        assert_eq!(meta.plot.as_deref(), Some("In a dystopian future..."));
        assert_eq!(meta.genres, vec!["Sci-Fi", "Thriller"]);
        assert_eq!(meta.tags, vec!["cyberpunk"]);
        assert_eq!(meta.studios, vec!["Warner Bros."]);
        assert_eq!(meta.rating, Some(8.1));
        assert_eq!(meta.critic_rating, Some(89.0));
        assert_eq!(meta.mpaa.as_deref(), Some("R"));
        assert_eq!(meta.status.as_deref(), Some("Released"));
        assert_eq!(meta.premiered.as_deref(), Some("1982-06-25"));
        assert_eq!(meta.runtime_ticks, Some(117 * jf_core::TICKS_PER_MINUTE));
        assert_eq!(meta.external_ids.get("imdb").map(String::as_str), Some("0083658"));
        assert_eq!(meta.external_ids.get("tmdb").map(String::as_str), Some("78"));
        assert_eq!(meta.actors.len(), 1);
        assert_eq!(meta.actors[0].name, "Harrison Ford");
        assert_eq!(meta.actors[0].role.as_deref(), Some("Deckard"));
        assert_eq!(meta.images.len(), 2);
        assert_eq!(meta.images[1].kind.as_deref(), Some("backdrop"));
        assert!(!meta.is_empty());
    }

    #[test]
    fn plot_prefers_full_over_outline() {
        let meta = parse_nfo_str(SAMPLE).unwrap();
        assert_eq!(meta.best_plot(), Some("In a dystopian future..."));
    }

    #[test]
    fn malformed_xml_is_an_error() {
        assert!(parse_nfo_str("<movie><title>unclosed").is_err());
    }

    #[test]
    fn empty_nfo_is_empty() {
        let meta = parse_nfo_str("<movie></movie>").unwrap();
        assert!(meta.is_empty());
    }

    #[test]
    fn tvshow_nfo_parses() {
        let xml = r#"<tvshow>
            <title>Severance</title>
            <year>2022</year>
            <genre>Drama</genre>
            <uniqueid type="tvdb">371980</uniqueid>
            <actor><name>Adam Scott</name></actor>
        </tvshow>"#;
        let meta = parse_nfo_str(xml).unwrap();
        assert_eq!(meta.title.as_deref(), Some("Severance"));
        assert_eq!(meta.year, Some(2022));
        assert_eq!(meta.external_ids.get("tvdb").map(String::as_str), Some("371980"));
        assert_eq!(meta.actors[0].name, "Adam Scott");
    }
}