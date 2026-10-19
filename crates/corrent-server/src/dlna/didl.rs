//! DIDL-Lite (the listing format inside Browse answers) and the little XML reading SOAP needs.

pub struct Item {
    pub id: String,
    pub parent: String,
    pub title: String,
    pub class: &'static str,
    pub year: Option<i32>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<i64>,
    pub duration_ms: Option<i64>,
    pub art: Option<String>,
    pub url: String,
    pub mime: String,
    pub size: u64,
}

pub enum Object {
    Container { id: String, parent: String, title: String, children: Option<i64> },
    Item(Item),
}

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            // Control characters aren't allowed in XML 1.0.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => o.push(c),
        }
    }
    o
}

fn unesc(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// The text of the first element named `name` (any namespace prefix).
pub fn tag(xml: &str, name: &str) -> Option<String> {
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let end = rest.find(['>', ' ', '\t', '\r', '\n', '/'])?;
        let tag = &rest[..end];
        let local = tag.rsplit_once(':').map(|(_, l)| l).unwrap_or(tag);
        if local == name && !tag.starts_with('/') {
            let close = rest.find('>')?;
            if rest[..close].ends_with('/') {
                return Some(String::new());
            }
            let body = &rest[close + 1..];
            let stop = body.find('<').unwrap_or(body.len());
            return Some(unesc(body[..stop].trim()));
        }
    }
    None
}

fn duration(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    format!("{}:{:02}:{:02}.{:03}", s / 3600, s / 60 % 60, s % 60, ms.max(0) % 1000)
}

pub fn document(objects: &[Object]) -> String {
    let mut o = String::from(
        r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/" xmlns:dlna="urn:schemas-dlna-org:metadata-1-0/">"#,
    );
    for obj in objects {
        match obj {
            Object::Container { id, parent, title, children } => {
                let count = children.map(|c| format!(r#" childCount="{c}""#)).unwrap_or_default();
                o.push_str(&format!(
                    r#"<container id="{}" parentID="{}" restricted="1" searchable="0"{count}><dc:title>{}</dc:title><upnp:class>object.container.storageFolder</upnp:class></container>"#,
                    esc(id),
                    esc(parent),
                    esc(title)
                ));
            }
            Object::Item(i) => {
                o.push_str(&format!(
                    r#"<item id="{}" parentID="{}" restricted="1"><dc:title>{}</dc:title><upnp:class>{}</upnp:class>"#,
                    esc(&i.id),
                    esc(&i.parent),
                    esc(&i.title),
                    i.class
                ));
                if let Some(y) = i.year {
                    o.push_str(&format!("<dc:date>{y:04}-01-01</dc:date>"));
                }
                if let Some(a) = &i.artist {
                    o.push_str(&format!("<upnp:artist>{0}</upnp:artist><dc:creator>{0}</dc:creator>", esc(a)));
                }
                if let Some(a) = &i.album {
                    o.push_str(&format!("<upnp:album>{}</upnp:album>", esc(a)));
                }
                if let Some(n) = i.track {
                    o.push_str(&format!("<upnp:originalTrackNumber>{n}</upnp:originalTrackNumber>"));
                }
                if let Some(a) = &i.art {
                    o.push_str(&format!("<upnp:albumArtURI>{}</upnp:albumArtURI>", esc(a)));
                }
                let dur = i.duration_ms.map(|d| format!(r#" duration="{}""#, duration(d))).unwrap_or_default();
                o.push_str(&format!(
                    r#"<res protocolInfo="http-get:*:{}:DLNA.ORG_OP=01;DLNA.ORG_CI=0" size="{}"{dur}>{}</res></item>"#,
                    esc(&i.mime),
                    i.size,
                    esc(&i.url)
                ));
            }
        }
    }
    o.push_str("</DIDL-Lite>");
    o
}
