//! One bounded entrypoint for local source metadata, including preflight reads.
use crate::{fail, Result};
use std::{fs::File, io::Read, path::Path};

pub(crate) const MAX_BYTES: usize = 1024 * 1024;

fn read_bounded(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return fail("source metadata exceeds 1 MiB");
    }
    Ok(bytes)
}

pub(crate) fn read(path: &Path) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    if file.metadata()?.len() > MAX_BYTES as u64 {
        return fail("source metadata exceeds 1 MiB");
    }
    // A file can grow after metadata() returns. The read itself remains capped.
    read_bounded(file)
}

pub(crate) fn json(path: &Path) -> Result<serde_json::Value> {
    Ok(serde_json::from_slice(&read(path)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, io, rc::Rc};

    struct Growing {
        consumed: Rc<Cell<usize>>,
    }
    impl Read for Growing {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            output.fill(b' ');
            self.consumed.set(self.consumed.get() + output.len());
            Ok(output.len())
        }
    }

    #[test]
    fn growing_source_stops_at_the_first_excess_byte() {
        let consumed = Rc::new(Cell::new(0));
        let error = read_bounded(Growing {
            consumed: consumed.clone(),
        })
        .unwrap_err();
        assert!(error.to_string().contains("exceeds 1 MiB"));
        assert_eq!(consumed.get(), MAX_BYTES + 1);
    }

    #[test]
    fn exact_limit_and_short_metadata_are_accepted() {
        let exact = vec![b' '; MAX_BYTES];
        assert_eq!(read_bounded(exact.as_slice()).unwrap().len(), MAX_BYTES);
        assert_eq!(read_bounded(b"{}".as_slice()).unwrap(), b"{}");
    }
}
