//! ICY metadata: with `Icy-MetaData: 1` in the request, Icecast and SHOUTcast servers
//! answer with `icy-metaint: N` and put a metadata block after every N bytes of audio: one
//! length byte (×16 bytes), then that many bytes of `StreamTitle='Artist - Title';...`
//! padded with NULs. A zero length byte means "no change".

/// Splits a stream's body into audio bytes and titles.
pub struct Icy {
    /// Audio bytes between metadata blocks; 0 when the server sends none.
    metaint: usize,
    /// Audio bytes left before the next length byte.
    audio_left: usize,
    /// Metadata bytes left in the block being read.
    meta_left: usize,
    meta: Vec<u8>,
}

impl Icy {
    pub fn new(metaint: usize) -> Icy {
        Icy {
            metaint,
            audio_left: metaint,
            meta_left: 0,
            meta: Vec::new(),
        }
    }

    /// Appends the audio in `input` to `audio`; returns the last title a metadata block in
    /// it carried (`Some("")` when the station cleared it).
    pub fn feed(&mut self, mut input: &[u8], audio: &mut Vec<u8>) -> Option<String> {
        if self.metaint == 0 {
            audio.extend_from_slice(input);
            return None;
        }
        let mut title = None;
        while !input.is_empty() {
            if self.meta_left > 0 {
                let n = self.meta_left.min(input.len());
                self.meta.extend_from_slice(&input[..n]);
                self.meta_left -= n;
                input = &input[n..];
                if self.meta_left == 0 {
                    if let Some(t) = stream_title(&self.meta) {
                        title = Some(t);
                    }
                    self.meta.clear();
                    self.audio_left = self.metaint;
                }
            } else if self.audio_left == 0 {
                self.meta_left = usize::from(input[0]) * 16;
                input = &input[1..];
                if self.meta_left == 0 {
                    self.audio_left = self.metaint;
                }
            } else {
                let n = self.audio_left.min(input.len());
                audio.extend_from_slice(&input[..n]);
                self.audio_left -= n;
                input = &input[n..];
            }
        }
        title
    }
}

/// The `StreamTitle` of a metadata block, if it has one.
pub fn stream_title(block: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(block);
    let text = text.trim_end_matches('\0');
    let rest = &text[text.find("StreamTitle='")? + "StreamTitle='".len()..];
    // The title may hold apostrophes; the field ends at `';` (or the block's last `'`).
    let end = rest.find("';").or_else(|| rest.rfind('\''))?;
    Some(rest[..end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A metadata block for `text`, padded to a multiple of 16 with NULs.
    fn block(text: &str) -> Vec<u8> {
        let len = text.len().div_ceil(16);
        let mut b = vec![len as u8];
        b.extend_from_slice(text.as_bytes());
        b.resize(1 + len * 16, 0);
        b
    }

    #[test]
    fn titles_parse() {
        let t = |s: &str| stream_title(s.as_bytes());
        assert_eq!(
            t("StreamTitle='Sea Band - Harbor Lights';StreamUrl='';\0\0"),
            Some("Sea Band - Harbor Lights".into())
        );
        assert_eq!(
            t("StreamTitle='Don't Look Back';"),
            Some("Don't Look Back".into())
        );
        assert_eq!(t("StreamTitle='';"), Some(String::new()));
        assert_eq!(t("StreamTitle='cut short"), None);
        assert_eq!(t("StreamUrl='x';"), None);
    }

    #[test]
    fn splits_audio_and_metadata_across_any_chunking() {
        // metaint 8: 8 audio bytes, a block, 8 audio bytes, an empty block, 3 audio bytes.
        let mut body: Vec<u8> = (0..8).collect();
        body.extend(block("StreamTitle='A - B';"));
        body.extend(8..16);
        body.push(0);
        body.extend(16..19);
        for chunk in 1..=body.len() {
            let mut icy = Icy::new(8);
            let mut audio = Vec::new();
            let mut titles = Vec::new();
            for part in body.chunks(chunk) {
                titles.extend(icy.feed(part, &mut audio));
            }
            assert_eq!(audio, (0..19).collect::<Vec<u8>>(), "chunks of {chunk}");
            assert_eq!(titles, ["A - B"], "chunks of {chunk}");
        }
    }

    #[test]
    fn without_metaint_everything_is_audio() {
        let mut icy = Icy::new(0);
        let mut audio = Vec::new();
        assert_eq!(icy.feed(&[1, 2, 0, 3], &mut audio), None);
        assert_eq!(audio, [1, 2, 0, 3]);
    }
}
