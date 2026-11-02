use std::io::Read;

use super::Rpf8Error;

/// Inflates raw deflate data (no zlib header), as .NET's `DeflateStream` writes.
pub fn inflate(data: &[u8], size_hint: usize) -> Result<Vec<u8>, Rpf8Error> {
    let mut out = Vec::with_capacity(size_hint);
    flate2::read::DeflateDecoder::new(data)
        .read_to_end(&mut out)
        .map_err(|e| Rpf8Error::Decompress(format!("deflate: {e}")))?;
    Ok(out)
}

/// Deflates to raw deflate data.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut enc, data).expect("writing to a Vec can't fail");
    enc.finish().expect("writing to a Vec can't fail")
}

/// Decompresses Oodle Kraken (or any other Oodle LZ format) to exactly `size` bytes.
pub fn oodle_decompress(data: &[u8], size: usize) -> Result<Vec<u8>, Rpf8Error> {
    let mut out = vec![0u8; size];
    // oozextract can still panic on some corrupt input; a bad file mustn't take
    // the whole program down.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        oozextract::Extractor::new().read_from_slice(data, &mut out)
    }))
    .map_err(|_| Rpf8Error::Decompress("oodle: corrupt data (decoder panicked)".into()))?;
    let n = result.map_err(|e| Rpf8Error::Decompress(format!("oodle: {e:?}")))?;
    if n != size {
        return Err(Rpf8Error::Decompress(format!("oodle: {n} bytes out, expected {size}")));
    }
    Ok(out)
}
