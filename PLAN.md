# Video Optimizer — proposed implementation plan

Status: initial single-file CLI implemented (Phase 1). Product defaults remain provisional. See README.md for the supported scope and known limitations. Phase 2–4 remain planned.

Implementation notes: dependency downloads were unavailable, so the first version uses a small native CLI parser, serde_json, and Unix signal handling through libc. Preset-based quality is explicitly unmeasured. Disk-space preflight and broader fixture coverage remain follow-ups; progress includes approximate speed and ETA.

## Product goal and limits

Build a local Rust CLI around installed FFmpeg and ffprobe. A single command should produce a smaller video with minimal visible degradation, explain its decisions, and preserve the original.

Lossy re-encoding cannot guarantee identical quality or smaller size for every input. Exact preservation requires lossless processing or stream copying, which often offers little size reduction. Quality scores measure similarity to the supplied source, not missing original detail. Enhancement changes the image and belongs in a separate opt-in workflow. Do not promise fixed percentage savings.

Borrow content-aware encoding principles from video streaming: inspect content, test settings, measure quality, and select a suitable encode. Adaptive streaming, multiple resolutions, and a YouTube-style delivery service are outside the initial scope.

## Decisions to confirm

1. Playback targets: modern devices and AV1, or broad compatibility and H.264?
2. Initial platforms: Linux first, or Linux/macOS/Windows together?
3. Time budget: slower compression, fast processing, or both modes?
4. Main inputs: phone recordings, screen recordings, movies, animation, or mixed?
5. Required special formats: HDR/Dolby Vision, multiple audio tracks, subtitles, and chapters?

Provisional direction: Linux first with portable Rust, modern-device AV1, quality-first CPU encoding, mixed SDR files, all supported tracks preserved. Unsupported special streams cause an actionable error rather than silent loss.

## CLI contract

Working executable name: `vopt`.

```text
vopt input.mp4
vopt input.mp4 -o output.mkv
vopt input.mp4 --profile compatible
vopt input.mp4 --effort thorough
vopt input.mp4 --quality strict
vopt input.mp4 --dry-run
vopt inspect input.mp4
vopt doctor
vopt input.mp4 --json
```

- Default invocation requires only an input path. Output is a sibling named `<stem>.optimized.<container>`.
- Profiles: `modern` uses AV1; `compatible` uses H.264 and MP4 subject to explicit track compatibility rules. HEVC is an advanced later option if needed.
- Quality policies: `high` and `strict` map to calibrated codec-specific search policies, not universal CRF numbers or promises of visual identity.
- Effort: `quick`, `balanced`, `thorough`; distinguish encoder effort from quality target. Quick mode reports when quality verification is limited.
- Select a container that preserves required streams. Explain the selection. Never silently drop a subtitle, audio track, or attachment to force MP4.
- Audio defaults to stream copy where supported. Audio conversion requires an explicit policy and never silently reduces multichannel audio to stereo.
- Show stages, progress, elapsed time, encoding speed, and approximate ETA. Report actual savings only after completion; label sample-based estimates.
- `--dry-run` reports decisions and arguments without encoding. Diagnostic logging and JSON support scripting; machine output goes to stdout and progress to stderr.
- Document stable statuses for success, skipped/no beneficial result, invalid input, missing dependency, encoding failure, validation failure, and cancellation.
- No overwrite by default. Explicit overwrite may replace a destination after validation, but the input path is never a valid destination.

## Processing pipeline

1. **Preflight:** locate FFmpeg/ffprobe; inspect versions, actual encoders, and filters; validate input/output identity, permissions, and available disk space. GPU encoder listings do not establish usable hardware.
2. **Probe:** parse ffprobe JSON into typed stream metadata: codecs, dimensions, pixel format, bit depth, duration, frame rates, rotation, color/HDR metadata, audio, subtitles, chapters, and dispositions. Identify unsupported cases before encoding.
3. **Plan:** select profile, encoder, container, stream mapping, and quality policy. Preserve resolution and timing by default. Do not upscale, denoise, sharpen, tone-map, or change frame rate automatically.
4. **Sample:** choose bounded clips across the timeline, including varied complexity when feasible. Encode candidate settings; measure aligned reference/candidate video using VMAF for supported SDR content. Use a bounded coarse-to-fine search and record all trials. Sample results are estimates, particularly for short complex scenes and final size.
5. **Encode:** run the selected full encode into a unique staging directory on the destination filesystem. Launch FFmpeg with argument arrays, never shell interpolation. Parse `-progress` and retain useful diagnostics.
6. **Validate:** check process status, output probing, expected stream inventory, duration/timing tolerance, dimensions, dispositions, and decodability. Enforce actual size reduction. Thorough mode measures full-video quality; sampled validation must be explicitly labeled. Use average and low-quality windows, not only a mean score. Limit retries if quality fails.
7. **Commit:** publish the validated file atomically where supported, respecting no-clobber semantics. If size or quality requirements fail, keep the original and report the reason. Optional diagnostics may retain rejected outputs only when explicitly requested.
8. **Report:** input/output sizes, savings, codec/container, elapsed time, validation coverage, quality measurements and their limits, and any intentional stream conversions.

Quality policy requires calibration before numerical thresholds become defaults. VMAF is optional at dependency level but required for a claim of VMAF verification. Without it, offer an explicitly unverified mode or installation guidance; do not silently substitute a differently scaled metric. HDR, unusual pixel formats, and interlaced inputs need separate supported policies; initially reject unsupported cases clearly.

## Rust architecture

Use one Cargo package with a binary entry point and testable library modules:

```text
src/main.rs           CLI wiring and exit handling
src/cli.rs            Commands, options, help, conflicts
src/capabilities.rs   Executable and encoder/filter discovery
src/probe.rs          ffprobe execution and typed metadata
src/policy.rs         Profiles, supported inputs, stream/container decisions
src/sampling.rs       Clip selection and bounded candidate search
src/quality.rs        Quality execution, alignment, score interpretation
src/encode.rs         FFmpeg command construction and execution
src/validate.rs       Structural, decode, size, and quality acceptance
src/job.rs            Pipeline, cancellation, staging and publication
src/report.rs         Human and versioned JSON reporting
```

Prefer subprocess integration over libav FFI initially: fewer native build/linking constraints, easy reproduction of commands, and use of the user's FFmpeg installation. Suggested crates are clap, serde/serde_json, indicatif, thiserror, tempfile, and a portable signal-handler crate. Choose versions compatible with the agreed Rust minimum; the local toolchain is Rust 1.85.0. Add an async runtime only if process orchestration or concurrency warrants it.

## Delivery phases and acceptance criteria

### Phase 1 — CLI and reliable single-file encoding

Create Cargo structure, help/examples, doctor/inspect, typed probing, explicit profiles, conservative stream rules, subprocess runner, progress, staging, cancellation, structural validation, size gate, and reporting.

Acceptance: one command processes supported SDR content; spaces and Unicode paths work; original bytes remain unchanged; existing destinations are protected; errors explain the remedy; a larger output is rejected. Quality is described as preset-based until measured search exists.

### Phase 2 — Content-aware quality selection

Implement representative clips, candidate encoding, aligned VMAF evaluation, bounded search, trial records, verification coverage reporting, and thorough full-video verification. Calibrate policies against a small varied corpus with visual review.

Acceptance: chosen settings and measurements are reproducible for the same configuration; no result is labeled fully verified based only on samples; failing quality or size results never replace a destination; retries and temporary disk use are bounded.

### Phase 3 — Workflow usability

Add directory input, bounded batch execution, optional recursive discovery, configuration defaults with documented precedence, per-file summaries, and batch failure isolation. Avoid oversubscribing CPU encoders by default. Add preview/comparison clips if useful after user feedback.

Acceptance: mixed batches finish with individual statuses; failures do not abort unrelated jobs; cancellation cleans staging; stable JSON allows scripts to consume results.

### Phase 4 — Broader support and release

Test macOS and Windows, package the Rust binary, document FFmpeg installation and required capabilities, and add platform CI. Review licenses before any bundled FFmpeg distribution. Add hardware acceleration only after runtime probing and quality/size benchmarks. Treat HDR support, enhancement, strict target-size encoding, and resume as separate extensions.

Acceptance: every advertised platform/profile has tested installation, encoding, cancellation, publication, and dependency-error behavior.

## Verification strategy

- Unit tests for policy decisions, argument construction, search bounds, score parsing, and destination safeguards.
- Fake-process tests for failed probes, encoder errors, truncated progress, cancellation, and malformed tool output.
- Small generated FFmpeg integration fixtures: audio/no audio, multiple tracks, subtitles, variable frame rate, rotation, odd dimensions, high bit depth, truncated input, and pre-existing output. Unsupported fixtures must fail before destructive action.
- Exercise source/destination aliasing, paths containing spaces/Unicode, full disk or failed writes where practical, and cleanup after cancellation.
- Benchmark a separate representative corpus: phone motion, static screen/text, animation, grain, low light, and already efficient encodes. Record size, runtime, quality distribution, and visual review. Synthetic clips alone cannot validate perceived quality.
- Run formatting, clippy, and tests in CI. Gate FFmpeg-dependent tests on declared capabilities and provide an explicit environment containing those dependencies.

## Confirmed local baseline

The repository contains no application files or existing project instructions. Rust 1.85.0 and Cargo 1.85.0 are installed. FFmpeg n9.0.1 and ffprobe are available; FFmpeg exposes libsvtav1, libx264, libx265, libvmaf, SSIM, and PSNR. Hardware encoder support is listed but device availability is unverified.

## References

- FFmpeg encoder documentation: https://ffmpeg.org/ffmpeg-codecs.html
- FFmpeg command and progress documentation: https://ffmpeg.org/ffmpeg.html
- FFmpeg quality filters: https://ffmpeg.org/ffmpeg-filters.html
- SVT-AV1 FFmpeg integration: https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/master/Docs/Ffmpeg.md
