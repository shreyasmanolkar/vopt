# Video Optimizer (`vout`)

A local Rust CLI that uses FFmpeg to create smaller videos while aiming for minimal visible quality loss. Your original is preserved, existing destinations are never overwritten, and larger results are discarded. The executable is named `vout`.

By default, the CLI uses quality-focused presets. `--auto` selects a setting using sample VMAF comparisons, and `--verify` measures the whole output. These metrics have provisional thresholds and cannot guarantee perceptual quality or text legibility. It does not restore missing detail or promise a particular size reduction. `thorough` means a slower encoder preset; add `--verify` to measure the full output.

## Install and run

Requires Rust 1.85+, FFmpeg, and ffprobe on `PATH`. FFmpeg must include `libsvtav1` for the modern profile or `libx264` for compatible, or `libx265` for HEVC. VMAF workflows also require FFmpeg with `libvmaf` and FFV1 support. Both tools must come from trusted installations. Linux is the tested platform. A manually triggered/tagged GitHub Actions workflow builds Linux, macOS and Windows artifacts, but those other platforms have not yet been validated end to end.

```bash
cargo build --release --locked
mkdir -p "$HOME/.local/bin"
install -m755 target/release/vopt "$HOME/.local/bin/vout"
vout doctor
vout video.mp4
```

`~/.local/bin` must be on your `PATH`. To confirm:

```bash
command -v vout
vout --help
```

## Examples

```bash
vout video.mp4                            # AV1 video, copied audio, MKV output
vout video.mp4 --profile compatible       # H.264 video, copied audio, MP4 output
vout video.mp4 --profile hevc             # H.265 video, copied audio, MP4 output
vout video.mp4 --analyze --content screen # Sample settings and estimate size
vout video.mp4 --auto --verify            # Choose on samples; verify full output
vout video.mp4 --analyze --preview ./clips # Save source/candidate sample pairs
vout batch ./recordings --recursive --auto # Process a folder sequentially
vout video.mp4 --threads 2 --config defaults.json
vout video.mp4 --quality strict           # Less aggressive compression
vout video.mp4 --effort thorough          # Slower preset
vout video.mp4 --effort quick             # Faster preset
vout video.mp4 -o smaller.mkv             # Explicit destination
vout video.mp4 --dry-run                  # Print arguments and decisions
vout inspect video.mp4                    # Show ffprobe metadata
vout doctor --json                        # Check dependency capabilities
vout video.mp4 --json                     # One JSON object on stdout
vout -- ./-unusual-filename.mp4           # Escape option-like filenames
```

Default output: `<input-stem>.optimized.mkv`, or `.mp4` for compatible, next to the resolved input file. Explicit destinations must have the profile's extension and an existing parent directory. A dry run probes input and dependencies but does not encode a video.

`high` and `strict` use separate codec-specific CRF values. With `--auto`, candidates are tested from highest CRF to lowest until every sample meets the selected provisional mean and fifth-percentile VMAF thresholds. `--content screen` applies stricter provisional thresholds; `camera` and `mixed` currently use the same thresholds. These are not calibrated visual-quality guarantees, and VMAF alone is insufficient to establish text readability. `quick`, `balanced`, and `thorough` control encoding effort independently. `--threads` sets the encoder and VMAF thread limit (1–64, default 4); it does not cap every FFmpeg decoder thread.

## Quality analysis and previews

`--analyze` takes one two-second sample for recordings up to six seconds and three two-second samples spaced across longer recordings. It creates lossless reference clips, tries a bounded set of encoder settings, and reports VMAF mean, fifth percentile, minimum, frame counts, the selected CRF, and rough size/time estimates. The estimate extrapolates sample video bytes and reported audio bitrate; sample container overhead, missing audio bitrate, changing content, and encoding startup can make it inaccurate. It excludes validation time.

`--preview DIR` saves the source and selected candidate clips as MKV files in an **existing** directory, without overwriting files. Use it with `--analyze` to examine clips without encoding the entire video. Samples contain video only. With `--auto`, selected settings are used for the full encode; the sample scores do not verify unsampled sections. `--verify` checks the entire output before publication and rejects results that fail the provisional VMAF thresholds. Frame counts and normalized timestamps must match within 5 ms. Full verification decodes both videos multiple times and can take considerable time and memory. VMAF is a similarity metric relative to the supplied source, not proof of equal quality; visually review dark scenes and screen text.

Quality analysis requires `libvmaf`. The program fails clearly if it is missing and never silently substitutes another metric. `--dry-run` with `--auto` shows the provisional command before sample selection. `--verify` may be used independently of `--auto` to check the preset-based encode.

## Batch jobs and defaults

`vout batch DIRECTORY` processes supported file extensions sequentially and produces a result for each file. `--recursive` includes subdirectories without following symlinks. Files ending in `.optimized` are skipped, and one failure does not stop the other files. The command exits with code 4 if any file failed. It uses automatic sibling output names; `--output` and `--preview` are not supported for batch jobs. No parallel batch scheduling or resume is available.

Pass a JSON defaults file explicitly with `--config defaults.json`. CLI options override matching values, regardless of argument order. Allowed keys are `profile`, `effort`, `quality`, `content`, `threads`, `auto`, and `verify`; unknown keys are errors. Boolean settings can be enabled on the CLI, but not disabled there, so omit them from the config when you need an unmeasured run.

```json
{"profile":"modern","effort":"balanced","quality":"strict","content":"screen","threads":4,"auto":true}
```

## Safety and validation

Encoding happens in a private staging directory on the destination filesystem. Before publishing, the CLI checks stream inventory, copied codecs, audio channel count/sample rate, video dimensions/color metadata, stream language/title/dispositions, chapters, and duration. It also fully decodes the output video and audio to check for errors. This is structural validation; `--verify` additionally performs the VMAF comparison described above.

Only an actually smaller, validated file is published. Atomic hard-link publication prevents overwriting a destination created during encoding; filesystems without hard-link support return an error and preserve the original. Ctrl+C or SIGTERM kills the active subprocess and removes staging files. SIGKILL/power failure can leave `.vopt-*` directories that may be manually removed once no job is running.

Audio is copied without recompression. Supported subtitle tracks and attachments are retained by the modern profile. MP4 forces a sole track of a type to be default in some cases; this specific container change is accepted and reported. Other disposition changes fail validation.

Have enough free disk space for an entire candidate output plus diagnostics and, for analysis, lossless temporary sample clips. The CLI warns if destination free space is below the original size, but space is not reserved and the warning does not predict the output size. Keep a backup independently for valuable media.

## Supported inputs and limits

- One regular file with one progressive, even-dimension, 8-bit 4:2:0 SDR video stream.
- No automatic resizing, frame-rate conversion, enhancement, or audio conversion.
- Modern/MKV copies common AAC, MP3, AC3/EAC3, Opus, Vorbis, FLAC, PCM s16/s24, DTS and TrueHD audio; SubRip, ASS/SSA, PGS/DVD subtitles; and attachments. Other streams are rejected.
- Compatible and HEVC/MP4 accept AAC, MP3 and AC3/EAC3 audio; subtitles, attachments and data tracks are rejected rather than silently dropped. Playback support still depends on the device, especially for audio codecs.
- HDR/high bit depth, interlacing, nonzero rotation metadata, multiple video streams/cover art, GPU encoding, and hardware quality verification are not yet supported.
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

The integration suite requires FFmpeg with libsvtav1, libx264, FFV1, FLAC and AAC. HEVC and VMAF tests are skipped if the relevant capability is unavailable. It generates its fixtures in a temporary directory and tests both profiles, subtitles, source preservation, rejection paths, and cancellation. This environment can also build using `--offline` with the cached dependencies.

See [PLAN.md](PLAN.md) for the roadmap. Sample selection, VMAF verification, and sequential batch workflows have initial implementations; calibration, GPU support, broader formats, and cross-platform releases remain future work. FFmpeg runs as a separate installed program and is not bundled.
