//! A test subtitle provider: one made-up subtitle per wanted language.

wit_bindgen::generate!({ path: "../../crates/corrent-plugins/wit", world: "subtitle-provider" });

use exports::corrent::plugin::subtitles::{Guest, Subtitle, SubtitleFile, SubtitleQuery};

struct Example;

impl Guest for Example {
    fn search(q: SubtitleQuery) -> Result<Vec<Subtitle>, String> {
        Ok(q.languages
            .iter()
            .map(|l| Subtitle {
                id: format!("{l}:{}", q.title),
                language: l.clone(),
                name: format!("{} ({l})", q.title),
                format: "srt".into(),
                downloads: Some(42),
                hearing_impaired: false,
                exact: q.file_hash.is_some(),
            })
            .collect())
    }

    fn download(id: String) -> Result<SubtitleFile, String> {
        let (lang, title) = id.split_once(':').ok_or("unknown subtitle")?;
        let text = format!("1\n00:00:01,000 --> 00:00:03,000\n{title} [{lang}]\n");
        Ok(SubtitleFile { format: "srt".into(), data: text.into_bytes() })
    }
}

export!(Example);
