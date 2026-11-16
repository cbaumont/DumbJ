use std::io::{BufRead, Read, Write};

pub(crate) const MAX_BODY: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("no Content-Length header")]
    MissingLength,
    #[error("invalid Content-Length header")]
    BadLength,
    #[error("message of {0} bytes is over the size limit")]
    TooLarge(usize),
    #[error("malformed header line")]
    BadHeader,
    #[error("stream ended inside a message")]
    Truncated,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Reads one message body. `Ok(None)` is a clean end of stream between messages.
pub(crate) fn read_frame(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, FrameError> {
    let mut length: Option<usize> = None;
    let mut first = true;
    loop {
        let mut line = Vec::new();
        let read = reader
            .by_ref()
            .take(MAX_HEADER_LINE)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return if first {
                Ok(None)
            } else {
                Err(FrameError::Truncated)
            };
        }
        first = false;
        if line.last() != Some(&b'\n') {
            return Err(if read as u64 >= MAX_HEADER_LINE {
                FrameError::BadHeader
            } else {
                FrameError::Truncated
            });
        }
        let line = line.trim_ascii_end();
        if line.is_empty() {
            break;
        }
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            return Err(FrameError::BadHeader);
        };
        if line[..colon].eq_ignore_ascii_case(b"content-length") {
            let value = std::str::from_utf8(&line[colon + 1..])
                .map_err(|_| FrameError::BadLength)?
                .trim();
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(FrameError::BadLength);
            }
            length = Some(value.parse().map_err(|_| FrameError::BadLength)?);
        }
    }
    let length = length.ok_or(FrameError::MissingLength)?;
    if length > MAX_BODY {
        return Err(FrameError::TooLarge(length));
    }
    let mut body = Vec::new();
    let got = reader.take(length as u64).read_to_end(&mut body)?;
    if got < length {
        return Err(FrameError::Truncated);
    }
    Ok(Some(body))
}

const MAX_HEADER_LINE: u64 = 8 * 1024;

pub(crate) fn write_frame(writer: &mut impl Write, body: &[u8]) -> std::io::Result<()> {
    let mut message = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    message.extend_from_slice(body);
    writer.write_all(&message)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn read(bytes: &[u8]) -> Result<Option<Vec<u8>>, FrameError> {
        read_frame(&mut std::io::BufReader::new(bytes))
    }

    #[test]
    fn reads_a_frame_with_extra_headers() {
        let input = b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length: 2\r\n\r\n{}";
        assert_eq!(read(input).unwrap(), Some(b"{}".to_vec()));
    }

    #[test]
    fn clean_end_is_none() {
        assert_eq!(read(b"").unwrap(), None);
    }

    #[test]
    fn rejects_bad_frames() {
        assert!(matches!(read(b"\r\n{}"), Err(FrameError::MissingLength)));
        assert!(matches!(
            read(b"Content-Length: x\r\n\r\n"),
            Err(FrameError::BadLength)
        ));
        assert!(matches!(
            read(b"Content-Length: -1\r\n\r\n"),
            Err(FrameError::BadLength)
        ));
        assert!(matches!(
            read(b"Content-Length: 99999999999\r\n\r\n"),
            Err(FrameError::TooLarge(_))
        ));
        assert!(matches!(
            read(b"Content-Length: 5\r\n\r\n{}"),
            Err(FrameError::Truncated)
        ));
        assert!(matches!(
            read(b"garbage\r\n\r\n"),
            Err(FrameError::BadHeader)
        ));
        assert!(matches!(
            read(b"Content-Length: 5"),
            Err(FrameError::Truncated)
        ));
    }

    #[test]
    fn reads_consecutive_frames() {
        let mut input = Vec::new();
        write_frame(&mut input, b"one").unwrap();
        write_frame(&mut input, b"two").unwrap();
        let mut reader = std::io::BufReader::new(input.as_slice());
        assert_eq!(read_frame(&mut reader).unwrap(), Some(b"one".to_vec()));
        assert_eq!(read_frame(&mut reader).unwrap(), Some(b"two".to_vec()));
        assert_eq!(read_frame(&mut reader).unwrap(), None);
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let mut reader = std::io::BufReader::new(bytes.as_slice());
            for _ in 0..4 {
                if !matches!(read_frame(&mut reader), Ok(Some(_))) {
                    break;
                }
            }
        }

        #[test]
        fn written_frames_round_trip(body in proptest::collection::vec(any::<u8>(), 0..2048)) {
            let mut buffer = Vec::new();
            write_frame(&mut buffer, &body).unwrap();
            prop_assert_eq!(read(&buffer).unwrap(), Some(body));
        }
    }
}
