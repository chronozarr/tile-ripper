# Scoped TIFF Zstandard decoder patch

Based on crates.io tiff-reader 0.9.0 (MIT OR Apache-2.0). This copy changes only the optional Zstandard filter from ruzstd streaming decoding to the already pinned zstd 0.13.3 one-shot decoder. TIFF parsing, predictors and block assembly remain upstream.

The decoder rejects trailing/concatenated frames and an advertised content size above the output limit. One-shot decompression writes to a fixed-capacity output and cannot allocate the frame-advertised streaming history window. Native COG admission retains its 2 MiB decoded tile and compressed-byte limits. Decoder context workspace is reserved separately by Tile-Ripper. Tests cover exact output, malformed/truncated frames, checksum failure, oversized output and trailing frames.
