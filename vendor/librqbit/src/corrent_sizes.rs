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
