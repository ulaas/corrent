//! Kodi `.nfo` files (movie, tvshow, episodedetails): existing libraries keep their edits.

use std::collections::BTreeMap;

use quick_xml::Reader;
use quick_xml::events::Event;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nfo {
    /// Root element: "movie", "tvshow", "episodedetails", "musicvideo", …
    pub kind: String,
    pub title: Option<String>,
    pub original_title: Option<String>,
    pub sort_title: Option<String>,
    pub year: Option<i32>,
    pub plot: Option<String>,
    pub rating: Option<f64>,
    pub mpaa: Option<String>,
    pub premiered: Option<String>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub runtime_min: Option<i64>,
    pub genres: Vec<String>,
    pub directors: Vec<String>,
    pub writers: Vec<String>,
    /// (name, role)
    pub actors: Vec<(String, Option<String>)>,
    pub ids: BTreeMap<String, String>,
}

const MAX_NFO: usize = 2 * 1024 * 1024;

/// Parses an .nfo; `None` when it isn't XML (some .nfo files are just a URL or release notes).
pub fn parse(text: &str) -> Option<Nfo> {
    if text.len() > MAX_NFO {
        return None;
    }
    let mut r = Reader::from_str(text);
    let mut n = Nfo::default();
    let mut path: Vec<String> = Vec::new();
    let mut uniqueid_type: Option<String> = None;
    let mut actor = (String::new(), None::<String>);
    // Text of the element being read (entities arrive as separate events).
    let mut buf = String::new();
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.local_name().as_ref().to_ascii_lowercase();
                if path.is_empty() {
                    n.kind = name.clone();
                }
                if name == "uniqueid" {
                    uniqueid_type = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.local_name().as_ref() == "type")
                        .map(|a| a.value.to_ascii_lowercase());
                }
                if name == "actor" {
                    actor = (String::new(), None);
                }
                path.push(name);
                buf.clear();
                if path.len() > 32 {
                    return None;
                }
            }
            Ok(Event::Text(t)) => buf.push_str(&t.xml10_content()),
            Ok(Event::CData(t)) => buf.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(g)) => {
                let name = g.xml10_content();
                buf.push_str(
                    &quick_xml::escape::unescape(&format!("&{name};")).map(|s| s.into_owned()).unwrap_or_default(),
                );
            }
            Ok(Event::End(_)) => {
                let v = std::mem::take(&mut buf).trim().to_string();
                let depth = path.len();
                let tag = path.last().cloned().unwrap_or_default();
                let parent = if depth >= 2 { path[depth - 2].clone() } else { String::new() };
                if tag == "actor" && !actor.0.is_empty() {
                    n.actors.push(std::mem::take(&mut actor));
                } else if !v.is_empty() && parent == "actor" {
                    match tag.as_str() {
                        "name" => actor.0 = v,
                        "role" => actor.1 = Some(v),
                        _ => {}
                    }
                } else if !v.is_empty() && depth == 2 {
                    field(&mut n, &tag, v, &mut uniqueid_type);
                }
                path.pop();
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
    }
    (!n.kind.is_empty()).then_some(n)
}

fn field(n: &mut Nfo, tag: &str, v: String, uniqueid_type: &mut Option<String>) {
    match tag {
        "title" => n.title = Some(v),
        "originaltitle" => n.original_title = Some(v),
        "sorttitle" => n.sort_title = Some(v),
        "year" => n.year = v.parse().ok(),
        "plot" | "outline" if n.plot.is_none() => n.plot = Some(v),
        "rating" => n.rating = v.replace(',', ".").parse().ok(),
        "mpaa" | "certification" => n.mpaa = Some(v),
        "premiered" | "aired" => {
            if n.year.is_none() {
                n.year = v.get(..4).and_then(|y| y.parse().ok());
            }
            n.premiered = Some(v);
        }
        "season" => n.season = v.parse().ok(),
        "episode" => n.episode = v.parse().ok(),
        "runtime" => n.runtime_min = v.parse().ok(),
        "genre" => n.genres.extend(v.split(['/', '|']).map(|g| g.trim().to_string()).filter(|g| !g.is_empty())),
        "director" => n.directors.push(v),
        "credits" | "writer" => n.writers.push(v),
        "uniqueid" => {
            let k = uniqueid_type.take().unwrap_or_else(|| "unknown".into());
            n.ids.insert(k, v);
        }
        "tmdbid" => {
            n.ids.insert("tmdb".into(), v);
        }
        "imdbid" | "imdb_id" => {
            n.ids.insert("imdb".into(), v);
        }
        "tvdbid" => {
            n.ids.insert("tvdb".into(), v);
        }
        "id" if v.starts_with("tt") => {
            n.ids.entry("imdb".into()).or_insert(v);
        }
        _ => {}
    }
}

/// The first line of `.nfo` files Corrent writes (it only ever overwrites its own).
pub const MARK: &str = "<!-- written by Corrent -->";

fn esc(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A Kodi-style `.nfo` (`movie`, `tvshow` or `episodedetails`).
pub fn render(n: &Nfo) -> String {
    let mut o = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n{MARK}\n<{}>\n", n.kind);
    let mut el = |tag: &str, v: &str| o.push_str(&format!("  <{tag}>{}</{tag}>\n", esc(v)));
    if let Some(v) = &n.title {
        el("title", v);
    }
    if let Some(v) = &n.original_title {
        el("originaltitle", v);
    }
    if let Some(v) = &n.sort_title {
        el("sorttitle", v);
    }
    if let Some(v) = n.year {
        el("year", &v.to_string());
    }
    if let Some(v) = n.season {
        el("season", &v.to_string());
    }
    if let Some(v) = n.episode {
        el("episode", &v.to_string());
    }
    if let Some(v) = &n.plot {
        el("plot", v);
    }
    if let Some(v) = n.rating {
        el("rating", &format!("{v:.1}"));
    }
    if let Some(v) = &n.mpaa {
        el("mpaa", v);
    }
    if let Some(v) = &n.premiered {
        el("premiered", v);
    }
    if let Some(v) = n.runtime_min {
        el("runtime", &v.to_string());
    }
    for g in &n.genres {
        el("genre", g);
    }
    for d in &n.directors {
        el("director", d);
    }
    for w in &n.writers {
        el("credits", w);
    }
    for (k, v) in &n.ids {
        o.push_str(&format!("  <uniqueid type=\"{}\">{}</uniqueid>\n", esc(k), esc(v)));
    }
    for (name, role) in &n.actors {
        o.push_str(&format!("  <actor>\n    <name>{}</name>\n", esc(name)));
        if let Some(r) = role {
            o.push_str(&format!("    <role>{}</role>\n", esc(r)));
        }
        o.push_str("  </actor>\n");
    }
    o.push_str(&format!("</{}>\n", n.kind));
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movie_nfo() {
        let n = parse(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<movie>
  <title>Sintel</title>
  <year>2010</year>
  <plot>A girl and a dragon.</plot>
  <rating>7,5</rating>
  <genre>Animation / Fantasy</genre>
  <director>Colin Levy</director>
  <uniqueid type="tmdb" default="true">45745</uniqueid>
  <uniqueid type="imdb">tt1727587</uniqueid>
  <actor><name>Halina Reijn</name><role>Sintel</role><thumb>x</thumb></actor>
  <set><name>Blender films</name></set>
</movie>"#,
        )
        .unwrap();
        assert_eq!(n.kind, "movie");
        assert_eq!((n.title.as_deref(), n.year, n.rating), (Some("Sintel"), Some(2010), Some(7.5)));
        assert_eq!(n.genres, vec!["Animation", "Fantasy"]);
        assert_eq!(n.ids.get("tmdb").map(String::as_str), Some("45745"));
        assert_eq!(n.ids.get("imdb").map(String::as_str), Some("tt1727587"));
        assert_eq!(n.actors, vec![("Halina Reijn".into(), Some("Sintel".into()))]);
        assert_eq!(n.directors, vec!["Colin Levy"]);
    }

    #[test]
    fn episode_and_garbage() {
        let n = parse("<episodedetails><title>Pilot</title><season>1</season><episode>1</episode><aired>2008-01-20</aired></episodedetails>").unwrap();
        assert_eq!((n.season, n.episode, n.year), (Some(1), Some(1), Some(2008)));
        assert!(parse("https://www.imdb.com/title/tt0133093/").is_none());
        let n = parse("<movie><title>Tom &amp; Jerry</title><plot><![CDATA[a <b> c]]></plot></movie>").unwrap();
        assert_eq!((n.title.as_deref(), n.plot.as_deref()), (Some("Tom & Jerry"), Some("a <b> c")));
        assert!(parse(&"<a>".repeat(100)).is_none());
    }

    #[test]
    fn render_round_trips() {
        let mut n = Nfo {
            kind: "movie".into(),
            title: Some("Sintel & <Friends>".into()),
            year: Some(2010),
            plot: Some("A girl and a dragon.".into()),
            genres: vec!["Animation".into()],
            directors: vec!["Colin Levy".into()],
            actors: vec![("Halina Reijn".into(), Some("Sintel".into()))],
            ..Default::default()
        };
        n.ids.insert("tmdb".into(), "45745".into());
        let text = render(&n);
        assert!(text.contains(MARK));
        let back = parse(&text).unwrap();
        assert_eq!(back.title, n.title);
        assert_eq!(back.year, Some(2010));
        assert_eq!(back.genres, n.genres);
        assert_eq!(back.directors, n.directors);
        assert_eq!(back.actors, n.actors);
        assert_eq!(back.ids.get("tmdb").map(String::as_str), Some("45745"));
    }
}
