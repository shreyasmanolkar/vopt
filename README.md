# Video Optimizer (`vopt`)

A local Rust CLI that uses FFmpeg to create smaller videos while aiming for minimal visible quality loss. Your original is preserved, existing destinations are never overwritten, and larger results are discarded.

This initial release uses quality-focused encoder presets. **Perceptual quality is not measured or guaranteed.** It does not restore missing detail, enhance images, or promise a particular size reduction. VMAF search and verification are planned next; `thorough` currently means a slower encoder preset, not quality verification.

## Install and run

Requires Rust 1.85+, FFmpeg, and ffprobe on `PATH`. FFmpeg must include `libsvtav1` for the modern profile or `libx264` for compatible. Both tools must come from trusted installations. Linux is the tested platform.

```bash
cargo build --release --locked
./target/release/vopt doctor
./target/release/vopt video.mp4
```

Optionally install the executable on your PATH:

```bash
cargo install --path . --locked
vopt --help
```

## Examples

```bash
vopt video.mp4                            # AV1 video, copied audio, MKV output
vopt video.mp4 --profile compatible       # H.264 video, copied audio, MP4 output
vopt video.mp4 --quality strict           # Less aggressive compression
vopt video.mp4 --effort thorough          # Slower preset
vopt video.mp4 --effort quick             # Faster preset
vopt video.mp4 -o smaller.mkv             # Explicit destination
vopt video.mp4 --dry-run                  # Print arguments and decisions
vopt inspect video.mp4                   # Show ffprobe metadata
vopt doctor --json                       # Check dependency capabilities
vopt video.mp4 --json                     # One JSON object on stdout
vopt -- ./-unusual-filename.mp4           # Escape option-like filenames
```

Default output: `<input-stem>.optimized.mkv`, or `.mp4` for compatible, next to the resolved input file. Explicit destinations must have the profile's extension and an existing parent directory. A dry run probes input and dependencies but does not encode a video.

`high` and `strict` use separate codec-specific CRF values. These are initial policies, not calibrated visual-quality guarantees. `quick`, `balanced`, and `thorough` control encoding effort independently. AV1 uses four logical processors; H.264 uses four encoding threads to avoid unbounded CPU use.

## Safety and validation

Encoding happens in a private staging directory on the destination filesystem. Before publishing, the CLI checks stream inventory, copied codecs, audio channel count/sample rate, video dimensions/color metadata, stream language/title/dispositions, chapters, and duration. It also fully decodes the output video and audio to check for errors. This is structural validation, not a perceptual comparison.

Only an actually smaller, validated file is published. Atomic hard-link publication prevents overwriting a destination created during encoding; filesystems without hard-link support return an error and preserve the original. Ctrl+C or SIGTERM kills the active subprocess and removes staging files. SIGKILL/power failure can leave `.vopt-*` directories that may be manually removed once no job is running.

Audio is copied without recompression. Supported subtitle tracks and attachments are retained by the modern profile. MP4 forces a sole track of a type to be default in some cases; this specific container change is accepted and reported. Other disposition changes fail validation.

Have enough free disk space for an entire candidate output plus diagnostics. Free space is not reserved or predicted. Keep a backup independently for valuable media.

## Supported inputs and limits

- One regular file with one progressive, even-dimension, 8-bit 4:2:0 SDR video stream.
- No automatic resizing, frame-rate conversion, enhancement, or audio conversion.
- Modern/MKV copies common AAC, MP3, AC3/EAC3, Opus, Vorbis, FLAC, PCM s16/s24, DTS and TrueHD audio; SubRip, ASS/SSA, PGS/DVD subtitles; and attachments. Other streams are rejected.
- Compatible/MP4 accepts AAC, MP3 and AC3/EAC3 audio; subtitles, attachments and data tracks are rejected rather than silently dropped. Playback support still depends on the device, especially for audio codecs.
- HDR/high bit depth, interlacing, nonzero rotation metadata, multiple video streams/cover art, batch processing, GPU encoding, and VMAF verification are not yet supported.
- Very efficient inputs may produce no smaller result. No candidate is kept in that case.
- Encoding progress, speed and ETA are approximate and exclude final validation.

JSON uses `schema_version: 1`; progress and diagnostic messages go to stderr. Large integer byte counts are reported without rounding. JSON paths/arguments use lossy display for non-UTF-8 names, although subprocess execution preserves native path bytes.

| Exit code | Meaning |
|---|---|
| 0 | Completed, inspected, dependency check succeeded, or plan printed |
| 2 | Invalid options/input, unsupported format, or existing destination |
| 3 | Missing executable or required encoder |
| 4 | Encoding, subprocess, filesystem, or publication failure |
| 5 | Output validation failed |
| 10 | Candidate was not smaller; no output published |
| 130 | Cancelled |

## Development

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python3 tests/smoke.py
```

The integration suite requires FFmpeg with libsvtav1, libx264, FFV1, FLAC and AAC. It generates its fixtures in a temporary directory and tests both profiles, subtitles, source preservation, rejection paths, and cancellation. This environment can also build using `--offline` with the cached dependencies.

See [PLAN.md](PLAN.md) for the roadmap. Phase 1 is implemented; quality-aware selection, batch workflows, and cross-platform releases remain future phases. FFmpeg runs as a separate installed program and is not bundled.
