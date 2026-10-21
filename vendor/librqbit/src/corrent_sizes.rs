// CORRENT PATCH (Z1): sizes librqbit-core's `Lengths` can't hold. `Lengths::from_torrent` sums the
// file lengths unchecked (a panic in debug builds, a wrong size in release), divides by the piece
// length (a piece length of 0 panics in every build) and keeps the piece count in a u32 (a count
// past it wraps, and `last_piece_id` underflows). Called before every `validate()` of a torrent's
// info: added from bytes, and received from peers for a magnet.

use librqbit_core::torrent_metainfo::TorrentMetaV1Info;

pub fn check<B>(info: &TorrentMetaV1Info<B>) -> anyhow::Result<()> {
    if info.piece_length == 0 {
        anyhow::bail!("invalid torrent: the piece length is 0");
    }
    let total = match &info.files {
        Some(files) => files.iter().try_fold(0u64, |sum, f| sum.checked_add(f.length)),
        None => Some(info.length.unwrap_or(0)),
    };
    let Some(total) = total else {
        anyhow::bail!("invalid torrent: the files add up to more than 16 EiB");
    };
    if total.div_ceil(u64::from(info.piece_length)) > u64::from(u32::MAX) {
        anyhow::bail!("invalid torrent: more than 4 billion pieces");
    }
    Ok(())
}

/// Select-only indices (`so=`) a magnet may list in all: librqbit-core's `Magnet::parse` expands
/// every range into a `Vec<usize>` (`so=0-18446744073709551615` asks for all of memory).
pub const MAX_SELECT_ONLY: u64 = 1 << 20;

/// [`Magnet::parse`] without `so=` parameters that would list more than [`MAX_SELECT_ONLY`]
/// indices (they are dropped: the magnet then selects every file).
pub fn parse_magnet(url: &str) -> anyhow::Result<librqbit_core::magnet::Magnet> {
    librqbit_core::magnet::Magnet::parse(&without_huge_select_only(url))
}

fn select_only_span(value: &str) -> u64 {
    value
        .split(',')
        .filter(|d| !d.is_empty())
        .map(|d| match d.split_once('-') {
            Some((a, b)) => match (a.parse::<usize>(), b.parse::<usize>()) {
                (Ok(a), Ok(b)) => (b as u64).saturating_sub(a as u64).saturating_add(u64::from(b >= a)),
                _ => 0,
            },
            None => 1,
        })
        .fold(0u64, u64::saturating_add)
}

fn without_huge_select_only(url: &str) -> std::borrow::Cow<'_, str> {
    let Some((head, query)) = url.split_once('?') else { return url.into() };
    let mut total = 0u64;
    let mut dropped = false;
    let kept: Vec<&str> = query
        .split('&')
        .filter(|piece| {
            // Decoded like `Url::query_pairs` does (`s%6F=` is `so=` too).
            let Some((key, value)) = url::form_urlencoded::parse(piece.as_bytes()).next() else { return true };
            if key != "so" {
                return true;
            }
            total = total.saturating_add(select_only_span(&value));
            let keep = total <= MAX_SELECT_ONLY;
            dropped |= !keep;
            keep
        })
        .collect();
    if !dropped {
        return url.into();
    }
    format!("{head}?{}", kept.join("&")).into()
}
