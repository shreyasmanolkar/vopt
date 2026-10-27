//! Bounded, opt-in quality search. All thresholds are provisional and do not
//! substitute for visual review, especially for small screen text.
use crate::{cli::Cli, media, process, Error, Result};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
    time::Instant,
};

pub struct Analysis {
    pub crf: u32,
    pub report: Value,
}

fn timestamps(path: &Path, log: &File) -> Result<Vec<f64>> {
    let mut a = media::args(&[
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_frames",
        "-show_entries",
        "frame=best_effort_timestamp_time",
        "-of",
        "csv=p=0",
    ]);
    a.push(path.as_os_str().to_owned());
    let data = process::run("ffprobe", &a, None, log)?;
    let text = String::from_utf8_lossy(&data);
    let mut times = Vec::new();
    for line in text.lines().filter(|s| !s.trim().is_empty()) {
        let time = line
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .parse::<f64>()
            .map_err(|_| Error::new(5, "Could not read all video frame timestamps"))?;
        if !time.is_finite() {
            return Err(Error::new(5, "Invalid frame timestamp"));
        }
        times.push(time);
    }
    Ok(times)
}

pub fn compare(
    distorted: &Path,
    reference: &Path,
    report: &Path,
    threads: u32,
    log: &File,
) -> Result<Value> {
    let reference_times = timestamps(reference, log)?;
    let distorted_times = timestamps(distorted, log)?;
    let reference_frames = reference_times.len() as u64;
    let distorted_frames = distorted_times.len() as u64;
    if reference_frames == 0 || reference_frames != distorted_frames {
        return Err(Error::new(5, format!("Cannot compare video: source has {reference_frames} frames and output has {distorted_frames}")));
    }
    let ref_start = reference_times[0];
    let dist_start = distorted_times[0];
    if reference_times
        .iter()
        .zip(&distorted_times)
        .any(|(r, d)| ((r - ref_start) - (d - dist_start)).abs() > 0.005)
    {
        return Err(Error::new(
            5,
            "Frame timestamps differ by over 5 ms; cannot reliably compare quality",
        ));
    }
    // Workspace path is created under the system temp directory. Filtergraph
    // escaping is separate from shell escaping: escape all special characters.
    let escaped = report
        .to_str()
        .ok_or_else(|| Error::new(2, "Quality log path must be UTF-8"))?
        .replace('\\', "\\\\")
        .replace(':', "\\:")
        .replace('\'', "\\'")
        .replace(',', "\\,")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace(';', "\\;");
    let filter = format!("[0:v:0]settb=1/1000,setpts=N[dist];[1:v:0]settb=1/1000,setpts=N[ref];[dist][ref]libvmaf=log_fmt=json:log_path='{escaped}':n_threads={threads}:shortest=1:repeatlast=0");
    let mut a = media::args(&["-hide_banner", "-nostdin", "-v", "error", "-xerror", "-i"]);
    a.push(distorted.as_os_str().to_owned());
    a.push("-i".into());
    a.push(reference.as_os_str().to_owned());
    a.extend(media::args(&[
        "-filter_complex",
        &filter,
        "-an",
        "-f",
        "null",
        "-",
    ]));
    process::run("ffmpeg", &a, None, log).map_err(|e| {
        Error::new(
            if e.code == 130 { 130 } else { 5 },
            format!("VMAF comparison failed: {}", e.message),
        )
    })?;
    let doc: Value = serde_json::from_slice(&fs::read(report).map_err(Error::io)?)
        .map_err(|e| Error::new(5, format!("Invalid VMAF report: {e}")))?;
    let mut scores: Vec<f64> = doc["frames"]
        .as_array()
        .ok_or_else(|| Error::new(5, "VMAF report has no frames"))?
        .iter()
        .map(|f| {
            f["metrics"]["vmaf"]
                .as_f64()
                .filter(|v| v.is_finite())
                .ok_or_else(|| Error::new(5, "Invalid VMAF frame score"))
        })
        .collect::<Result<_>>()?;
    if scores.len() as u64 != reference_frames {
        return Err(Error::new(
            5,
            format!("VMAF covered {} of {reference_frames} frames", scores.len()),
        ));
    }
    let mean = scores.iter().sum::<f64>() / scores.len() as f64;
    scores.sort_by(f64::total_cmp);
    let p05 = scores[((scores.len() - 1) as f64 * 0.05) as usize];
    Ok(
        json!({"mean":mean,"p05":p05,"minimum":scores[0],"frames":scores.len(),"metric":"vmaf_v0.6.1"}),
    )
}

fn targets(content: &str, quality: &str) -> (f64, f64) {
    match (content, quality) {
        ("screen", "strict") => (97.0, 93.0),
        ("screen", _) => (95.0, 90.0),
        (_, "strict") => (96.0, 92.0),
        _ => (93.0, 88.0),
    }
}
pub fn enforce(score: &Value, content: &str, quality: &str) -> Result<()> {
    let (mean, p05) = targets(content, quality);
    let actual_mean = score["mean"].as_f64().unwrap_or(-1.0);
    let actual_p05 = score["p05"].as_f64().unwrap_or(-1.0);
    if actual_mean < mean || actual_p05 < p05 {
        return Err(Error::new(5,format!("Quality below provisional target: VMAF mean {actual_mean:.2} (target {mean}), 5th percentile {actual_p05:.2} (target {p05})")));
    }
    Ok(())
}

fn copy_new(from: &Path, to: &Path) -> Result<()> {
    let mut source = File::open(from).map_err(Error::io)?;
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|e| Error::new(4, format!("Cannot create preview {}: {e}", to.display())))?;
    if let Err(e) = io::copy(&mut source, &mut destination) {
        drop(destination);
        let _ = fs::remove_file(to);
        return Err(Error::io(e));
    }
    Ok(())
}

pub fn analyze(c: &Cli, input: &Path, info: &Value, work: &Path, log: &File) -> Result<Analysis> {
    let duration = media::duration(info)?;
    let clip_length = duration.min(2.0);
    let count = if duration <= 6.0 { 1 } else { 3 };
    let starts: Vec<f64> = if count == 1 {
        vec![0.0]
    } else {
        [0.1, 0.5, 0.9]
            .iter()
            .map(|p| ((duration - clip_length) * p).max(0.0))
            .collect()
    };
    let mut refs = Vec::new();
    for (i, offset) in starts.iter().enumerate() {
        let reference = work.join(format!("reference-{i}.mkv"));
        eprintln!(
            "Preparing sample {}/{} at {:.1}s…",
            i + 1,
            starts.len(),
            offset
        );
        let mut a = media::args(&[
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-xerror",
            "-ss",
            &format!("{offset:.6}"),
            "-i",
        ]);
        a.push(input.as_os_str().to_owned());
        a.extend(media::args(&[
            "-t",
            &format!("{clip_length:.6}"),
            "-map",
            "0:v:0",
            "-an",
            "-c:v",
            "ffv1",
            "-threads",
            &c.threads.to_string(),
            "-fps_mode",
            "passthrough",
            "-n",
        ]));
        a.push(reference.as_os_str().to_owned());
        process::run("ffmpeg", &a, None, log)?;
        refs.push(reference);
    }
    let candidates: &[u32] = match c.profile.as_str() {
        // Keep testing at finer intervals near the high-quality end. A source
        // can narrowly miss the target at the old floor while a modestly lower
        // CRF still produces a useful, smaller result.
        "modern" => &[34, 28, 22, 16, 14, 12],
        "hevc" => &[30, 26, 22, 18, 16, 14],
        _ => &[26, 22, 18, 14, 12, 10],
    };
    let preset = match (c.profile.as_str(), c.effort.as_str()) {
        ("modern", "quick") => "10",
        ("modern", "thorough") => "4",
        ("modern", _) => "7",
        (_, "quick") => "veryfast",
        (_, "thorough") => "slow",
        _ => "medium",
    };
    let encoder = match c.profile.as_str() {
        "modern" => "libsvtav1",
        "hevc" => "libx265",
        _ => "libx264",
    };
    let mut trials = Vec::new();
    for crf in candidates {
        eprintln!("Testing {encoder} CRF {crf} on {} sample(s)…", refs.len());
        let mut scores = Vec::new();
        let mut bytes = 0u64;
        let mut encode_seconds = 0.0;
        for (i, reference) in refs.iter().enumerate() {
            let output = work.join(format!("candidate-{crf}-{i}.mkv"));
            let mut a = media::args(&["-hide_banner", "-nostdin", "-v", "error", "-xerror", "-i"]);
            a.push(reference.as_os_str().to_owned());
            a.extend(media::args(&[
                "-map",
                "0:v:0",
                "-an",
                "-c:v",
                encoder,
                "-preset",
                preset,
                "-crf",
                &crf.to_string(),
                "-pix_fmt",
                "yuv420p",
                "-fps_mode",
                "passthrough",
            ]));
            if c.profile == "modern" {
                a.extend(media::args(&[
                    "-svtav1-params",
                    &format!("lp={}", c.threads),
                ]));
            } else {
                if c.profile == "hevc" {
                    a.extend(media::args(&[
                        "-x265-params",
                        &format!("pools={}:frame-threads=1", c.threads),
                    ]));
                }
                a.extend(media::args(&["-threads", &c.threads.to_string()]));
            }
            a.push(output.as_os_str().to_owned());
            let encode_start = Instant::now();
            process::run("ffmpeg", &a, None, log)?;
            encode_seconds += encode_start.elapsed().as_secs_f64();
            bytes += fs::metadata(&output).map_err(Error::io)?.len();
            scores.push(compare(
                &output,
                reference,
                &work.join(format!("vmaf-{crf}-{i}.json")),
                c.threads,
                log,
            )?);
        }
        let passed = scores
            .iter()
            .all(|s| enforce(s, &c.content, &c.quality).is_ok());
        trials
            .push(json!({"crf":crf,"passed":passed,"scores":scores,"sample_encoded_bytes":bytes}));
        if passed {
            if let Some(dir) = &c.preview {
                if !dir.is_dir() {
                    return Err(Error::new(2, "Preview directory must already exist"));
                }
                let mut created = Vec::new();
                for (i, reference) in refs.iter().enumerate() {
                    for (source, name) in [
                        (reference.clone(), format!("sample-{i}-source.mkv")),
                        (
                            work.join(format!("candidate-{crf}-{i}.mkv")),
                            format!("sample-{i}-encoded.mkv"),
                        ),
                    ] {
                        let dest = dir.join(name);
                        if let Err(e) = copy_new(&source, &dest) {
                            for p in created {
                                let _ = fs::remove_file(p);
                            }
                            return Err(e);
                        }
                        created.push(dest);
                    }
                }
            }
            // Candidate sizes contain MKV overhead and exclude audio. Add an
            // estimate from the reported audio bitrates; all estimates are rough.
            let audio_bps: u64 = media::streams(info)?
                .iter()
                .filter(|s| s["codec_type"] == "audio")
                .filter_map(|s| s["bit_rate"].as_str()?.parse::<u64>().ok())
                .sum();
            let estimated = (bytes as f64 / (clip_length * refs.len() as f64) * duration
                + audio_bps as f64 / 8.0 * duration) as u64;
            let estimate_runtime = encode_seconds / (clip_length * refs.len() as f64) * duration;
            let (mean, p05) = targets(&c.content, &c.quality);
            return Ok(Analysis {
                crf: *crf,
                report: json!({"schema_version":1,"status":"analyzed","input":input.to_string_lossy(),"content":c.content,"selected_crf":crf,"encoder":encoder,"sample_offsets_seconds":starts,"sample_seconds":clip_length,"trials":trials,"targets":{"mean":mean,"p05":p05,"provisional":true},"estimated_output_bytes":estimated,"estimated_full_encode_seconds":estimate_runtime,"estimates":"rough; include sample overhead, exclude validation, and may differ substantially from full video","quality_verification":"sample_vmaf","preview_directory":c.preview.as_ref().map(|p|p.to_string_lossy()),"notice":"VMAF compares against the source; inspect previews, especially screen text and dark scenes."}),
            });
        }
    }
    let (mean, p05) = targets(&c.content, &c.quality);
    Err(Error::new(5, format!("No sampled setting met the provisional quality targets (content={}, quality={}, mean VMAF >= {mean}, p05 >= {p05}); no output was published. Trials: {}", c.content, c.quality, serde_json::to_string(&trials).unwrap_or_default())))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thresholds_reject_bad_percentile() {
        assert!(enforce(&json!({"mean":99.0,"p05":60.0}), "screen", "high").is_err());
        assert!(enforce(&json!({"mean":99.0,"p05":98.0}), "screen", "strict").is_ok());
    }
}
