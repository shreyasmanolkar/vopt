mod cli;
mod media;
mod process;
mod quality;

use cli::Cli;
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Error>;
#[derive(Debug)]
pub struct Error {
    code: i32,
    message: String,
}
impl Error {
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    fn io(e: std::io::Error) -> Self {
        Self::new(4, e.to_string())
    }
}

struct Workspace(PathBuf);
impl Workspace {
    fn new(parent: &Path) -> Result<Self> {
        for n in 0..100 {
            let path = parent.join(format!(
                ".vopt-{}-{}-{n}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(Error::io(e)),
            }
        }
        Err(Error::new(4, "Cannot create staging directory"))
    }
    fn log(&self) -> Result<File> {
        OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(self.0.join("diagnostics.log"))
            .map_err(Error::io)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn discover(log: &File) -> Result<Value> {
    let version = |name: &str| -> Result<String> {
        let bytes = process::run(name, &media::args(&["-version"]), None, log)?;
        Ok(String::from_utf8_lossy(&bytes)
            .lines()
            .next()
            .unwrap_or("")
            .to_owned())
    };
    let ffmpeg = version("ffmpeg")?;
    let ffprobe = version("ffprobe")?;
    let encoders = process::run(
        "ffmpeg",
        &media::args(&["-hide_banner", "-encoders"]),
        None,
        log,
    )?;
    let encoders = String::from_utf8_lossy(&encoders);
    let has = |name: &str| {
        encoders
            .lines()
            .any(|l| l.split_whitespace().nth(1) == Some(name))
    };
    let filters = process::run(
        "ffmpeg",
        &media::args(&["-hide_banner", "-filters"]),
        None,
        log,
    )?;
    Ok(
        json!({"schema_version":1,"status":"ok","ffmpeg":ffmpeg,"ffprobe":ffprobe,"encoders":{"libsvtav1":has("libsvtav1"),"libx264":has("libx264"),"libx265":has("libx265")},"vmaf_available":String::from_utf8_lossy(&filters).lines().any(|l| l.split_whitespace().nth(1)==Some("libvmaf")),"quality_verification":"optional_vmaf"}),
    )
}

fn output_path(input: &Path, requested: Option<&Path>, ext: &str) -> Result<PathBuf> {
    let output = requested.map(Path::to_path_buf).unwrap_or_else(|| {
        let mut name = input.file_stem().unwrap_or_default().to_os_string();
        name.push(format!(".optimized.{ext}"));
        input.with_file_name(name)
    });
    if output.extension().and_then(|x| x.to_str()) != Some(ext) {
        return Err(Error::new(
            2,
            format!("This profile requires a .{ext} output"),
        ));
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent
        .canonicalize()
        .map_err(|e| Error::new(2, format!("Output directory is unavailable: {e}")))?;
    let output = parent.join(
        output
            .file_name()
            .ok_or_else(|| Error::new(2, "Output filename is required"))?,
    );
    if output.symlink_metadata().is_ok() {
        return Err(Error::new(
            2,
            "Destination already exists; choose another output path",
        ));
    }
    if output == input {
        return Err(Error::new(2, "Input cannot be the output"));
    }
    Ok(output)
}

fn encoding_args(
    c: &Cli,
    input: &Path,
    output: &Path,
    info: &Value,
    selected_crf: Option<u32>,
) -> Result<Vec<OsString>> {
    let modern = c.profile == "modern";
    let preset = match (modern, c.effort.as_str()) {
        (true, "quick") => "10",
        (true, "thorough") => "4",
        (true, _) => "7",
        (false, "quick") => "veryfast",
        (false, "thorough") => "slow",
        _ => "medium",
    };
    let crf = match (modern, c.quality.as_str()) {
        (true, "strict") => "20",
        (true, _) => "28",
        (false, "strict") => "16",
        _ => "20",
    };
    let crf = if c.profile == "hevc" {
        if c.quality == "strict" {
            "20"
        } else {
            "24"
        }
    } else {
        crf
    };
    let crf = selected_crf
        .map(|n| n.to_string())
        .unwrap_or_else(|| crf.to_string());
    let mut a = media::args(&[
        "-hide_banner",
        "-nostdin",
        "-v",
        "warning",
        "-n",
        "-noautorotate",
        "-i",
    ]);
    a.push(input.as_os_str().to_owned());
    a.extend(media::args(&[
        "-map",
        "0",
        "-map_metadata",
        "0",
        "-map_chapters",
        "0",
        "-c",
        "copy",
        "-c:v:0",
        match c.profile.as_str() {
            "modern" => "libsvtav1",
            "hevc" => "libx265",
            _ => "libx264",
        },
        "-preset",
        preset,
        "-crf",
        &crf,
        "-pix_fmt",
        "yuv420p",
        "-fps_mode:v:0",
        "passthrough",
    ]));
    // Avoid FFmpeg's automatic default-disposition assignment on copied streams.
    for (index, s) in media::streams(info)?.iter().enumerate() {
        let flags = s["disposition"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter(|(_, v)| v.as_i64() == Some(1))
                    .map(|(k, _)| k.as_str())
                    .collect::<Vec<_>>()
                    .join("+")
            })
            .unwrap_or_default();
        a.push(format!("-disposition:{index}").into());
        a.push(if flags.is_empty() {
            "0".into()
        } else {
            flags.into()
        });
    }
    if modern {
        a.extend(media::args(&[
            "-svtav1-params",
            &format!("lp={}", c.threads),
        ]));
    } else {
        if c.profile == "hevc" {
            a.extend(media::args(&[
                "-x265-params",
                &format!("pools={}:frame-threads=1", c.threads),
                "-tag:v",
                "hvc1",
            ]));
        }
        a.extend(media::args(&[
            "-threads",
            &c.threads.to_string(),
            "-movflags",
            "+faststart",
        ]));
    }
    a.extend(media::args(&["-progress", "pipe:1", "-nostats"]));
    a.push(output.as_os_str().to_owned());
    Ok(a)
}

#[cfg(unix)]
fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return None;
    }
    let stats = unsafe { stats.assume_init() };
    Some(stats.f_bavail.saturating_mul(stats.f_frsize))
}
#[cfg(not(unix))]
fn free_bytes(_path: &Path) -> Option<u64> {
    None
}

fn execute(c: &Cli) -> Result<Value> {
    let scratch = Workspace::new(&std::env::temp_dir())?;
    let log = scratch.log()?;
    if c.command == "doctor" {
        return discover(&log);
    }
    let input = c
        .input
        .as_ref()
        .unwrap()
        .canonicalize()
        .map_err(|e| Error::new(2, format!("Cannot open input: {e}")))?;
    if !input.is_file() {
        return Err(Error::new(2, "Input must be a regular file"));
    }
    let info = media::probe(&input, &log)?;
    if c.command == "inspect" {
        return Ok(
            json!({"schema_version":1,"status":"ok","input":input.to_string_lossy(),"media":info}),
        );
    }
    let modern = c.profile == "modern";
    media::check(&info, !modern)?;
    let caps = discover(&log)?;
    let encoder = match c.profile.as_str() {
        "modern" => "libsvtav1",
        "hevc" => "libx265",
        _ => "libx264",
    };
    if caps["encoders"][encoder] != true {
        return Err(Error::new(3,format!("Your FFmpeg lacks {encoder}; install a build containing it or choose another profile")));
    }
    let ext = if modern { "mkv" } else { "mp4" };
    let analysis_output = scratch.0.join(format!("unused.{ext}"));
    let output = if c.analyze {
        output_path(&input, Some(&analysis_output), ext)?
    } else {
        output_path(&input, c.output.as_deref(), ext)?
    };
    let planned = encoding_args(c, &input, &output, &info, None)?;
    if c.dry_run {
        return Ok(
            json!({"schema_version":1,"status":"planned","input":input.to_string_lossy(),"output":output.to_string_lossy(),"profile":c.profile,"quality":c.quality,"quality_verification":if c.verify {"full_vmaf_requested"} else if c.auto {"sample_vmaf_requested"} else {"unmeasured_presets"},"program":"ffmpeg","arguments":planned.iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>(),"note":"Actual encoding uses a staging path before publication; no video will be encoded by dry-run."}),
        );
    }
    if (c.auto || c.verify) && caps["vmaf_available"] != true {
        return Err(Error::new(
            3,
            "This feature requires FFmpeg built with libvmaf; run vopt doctor",
        ));
    }
    let analysis = if c.auto {
        Some(quality::analyze(c, &input, &info, &scratch.0, &log)?)
    } else {
        None
    };
    if c.analyze {
        return Ok(analysis.unwrap().report);
    }
    let selected_crf = analysis.as_ref().map(|a| a.crf);
    if let Some(free) = free_bytes(output.parent().unwrap()) {
        let source_size = fs::metadata(&input).map_err(Error::io)?.len();
        if free < source_size {
            eprintln!("Warning: destination has {free} free bytes, less than the source size {source_size}. Space for the candidate is not guaranteed.");
        }
    }
    let stage = Workspace::new(output.parent().unwrap())?;
    let encoded = stage.0.join(format!("encoded.{ext}"));
    let a = encoding_args(c, &input, &encoded, &info, selected_crf)?;
    eprintln!(
        "Encoding {} with {encoder}; {}.",
        input.display(),
        if c.auto {
            "sample quality measured; full output not yet verified"
        } else {
            "quality is preset-based, not measured"
        }
    );
    let start = Instant::now();
    process::run("ffmpeg", &a, Some(media::duration(&info)?), &log)?;
    eprintln!("Validating streams and decoding output…");
    let result = media::probe(&encoded, &log)
        .map_err(|e| Error::new(if e.code == 130 { 130 } else { 5 }, e.message))?;
    let notices = media::validate(&info, &result, !modern)?;
    let mut decode = media::args(&["-hide_banner", "-nostdin", "-v", "error", "-xerror", "-i"]);
    decode.push(encoded.as_os_str().to_owned());
    decode.extend(media::args(&[
        "-map", "0:v:0", "-map", "0:a?", "-f", "null", "-",
    ]));
    process::run("ffmpeg", &decode, None, &log)
        .map_err(|e| Error::new(if e.code == 130 { 130 } else { 5 }, e.message))?;
    let verification = if c.verify {
        eprintln!("Measuring full-video VMAF; this may take a while…");
        let score = quality::compare(
            &encoded,
            &input,
            &scratch.0.join("full-vmaf.json"),
            c.threads,
            &log,
        )?;
        quality::enforce(&score, &c.content, &c.quality)?;
        Some(score)
    } else {
        None
    };
    let original_bytes = fs::metadata(&input).map_err(Error::io)?.len();
    let output_bytes = fs::metadata(&encoded).map_err(Error::io)?.len();
    let smaller = output_bytes < original_bytes;
    if smaller {
        if process::CANCELLED.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Error::new(130, "Cancelled"));
        }
        File::open(&encoded)
            .map_err(Error::io)?
            .sync_all()
            .map_err(Error::io)?;
        // Atomic no-clobber publication on the same filesystem; an existing destination
        // (including a symlink created since preflight) can never be overwritten.
        fs::hard_link(&encoded, &output)
            .map_err(|e| Error::new(4, format!("Cannot publish output safely: {e}")))?;
    }
    Ok(
        json!({"schema_version":1,"status":if smaller {"optimized"} else {"skipped"},"reason":if smaller {"smaller_validated_output"} else {"no_size_reduction"},"input":input.to_string_lossy(),"output":if smaller {Some(output.to_string_lossy())} else {None},"input_bytes":original_bytes,"candidate_bytes":output_bytes,"savings_percent":100.0*(1.0-output_bytes as f64/original_bytes as f64),"elapsed_seconds":start.elapsed().as_secs_f64(),"encoder":encoder,"quality_verification":if c.verify {"full_vmaf"} else if c.auto {"sample_vmaf"} else {"unmeasured_presets"},"analysis":analysis.map(|a| a.report),"vmaf":verification,"validation":{"stream_inventory":true,"decode":true},"notices":notices}),
    )
}

fn batch(c: &Cli) -> Result<Value> {
    fn collect(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(dir)
            .map_err(|e| Error::new(2, format!("Cannot read {}: {e}", dir.display())))?
        {
            let entry = entry.map_err(Error::io)?;
            let kind = entry.file_type().map_err(Error::io)?;
            let path = entry.path();
            if kind.is_dir() && recursive {
                collect(&path, recursive, out)?;
            } else if kind.is_file() {
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                if matches!(ext.as_str(), "mp4" | "mkv" | "mov" | "webm" | "avi")
                    && !stem.ends_with(".optimized")
                {
                    out.push(path);
                }
            }
        }
        Ok(())
    }
    let dir = c.input.as_ref().unwrap();
    if !dir.is_dir() {
        return Err(Error::new(2, "batch input must be a directory"));
    }
    let mut paths = Vec::new();
    collect(dir, c.recursive, &mut paths)?;
    paths.sort();
    if paths.is_empty() {
        return Err(Error::new(2, "No video files found"));
    }
    let mut results = Vec::new();
    let mut failures = 0;
    for (index, path) in paths.iter().enumerate() {
        if process::CANCELLED.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Error::new(130, "Batch cancelled"));
        }
        eprintln!("Batch {}/{}: {}", index + 1, paths.len(), path.display());
        let mut job = c.clone();
        job.command = "optimize".into();
        job.input = Some(path.clone());
        job.recursive = false;
        match execute(&job) {
            Ok(v) => results.push(v),
            Err(e) if e.code == 130 => return Err(e),
            Err(e) => {
                failures += 1;
                results.push(json!({"status":"error","input":path.to_string_lossy(),"exit_code":e.code,"message":e.message}));
            }
        }
    }
    Ok(
        json!({"schema_version":1,"status":if failures == 0 {"batch_complete"} else {"batch_partial"},"files":results,"failed":failures}),
    )
}

fn main() {
    process::install_signals();
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let json_requested = args.iter().any(|s| s == "--json");
    let result = Cli::parse(args)
        .map_err(|e| Error::new(2, e))
        .and_then(|c| {
            if c.command == "help" {
                print!("{}", cli::HELP);
                return Ok(None);
            }
            if c.command == "version" {
                println!("vopt {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            (if c.command == "batch" {
                batch(&c)
            } else {
                execute(&c)
            })
            .map(|v| Some((c.json, v)))
        });
    let code = match result {
        Ok(Some((machine, v))) => {
            if !machine {
                if let Some(notices) = v["notices"].as_array() {
                    for notice in notices {
                        eprintln!("Note: {}", notice.as_str().unwrap_or(""));
                    }
                }
            }
            if machine {
                println!("{}", serde_json::to_string(&v).unwrap());
            } else if v["status"] == "optimized" {
                println!(
                    "Saved {:.1}%: {} → {} bytes\nOutput: {}\nQuality verification: {}",
                    v["savings_percent"].as_f64().unwrap(),
                    v["input_bytes"],
                    v["candidate_bytes"],
                    v["output"].as_str().unwrap_or(""),
                    v["quality_verification"].as_str().unwrap_or("unknown")
                );
            } else if v["status"] == "skipped" {
                println!(
                    "No smaller result; original preserved. Candidate: {} bytes, input: {} bytes.",
                    v["candidate_bytes"], v["input_bytes"]
                );
            } else {
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
            if v["status"] == "skipped" {
                10
            } else if v["status"] == "batch_partial" {
                4
            } else {
                0
            }
        }
        Ok(None) => 0,
        Err(e) => {
            if json_requested {
                println!(
                    "{}",
                    json!({"schema_version":1,"status":"error","exit_code":e.code,"message":e.message})
                );
            } else {
                eprintln!("Error: {}", e.message);
            }
            e.code
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_existing_output() {
        let w = Workspace::new(&std::env::temp_dir()).unwrap();
        let out = w.0.join("exists.mkv");
        fs::write(&out, b"keep").unwrap();
        assert!(output_path(Path::new("input.mp4"), Some(&out), "mkv").is_err());
        assert_eq!(fs::read(out).unwrap(), b"keep");
    }
    #[test]
    fn staging_is_cleaned() {
        let p;
        {
            let w = Workspace::new(&std::env::temp_dir()).unwrap();
            p = w.0.clone();
            fs::write(p.join("partial"), b"x").unwrap();
        }
        assert!(!p.exists());
    }
    #[test]
    fn quality_is_codec_specific() {
        let c = Cli::parse(["input.mp4", "--quality", "strict"].map(OsString::from)).unwrap();
        let a = encoding_args(
            &c,
            Path::new("/tmp/input with spaces.mp4"),
            Path::new("/tmp/out.mkv"),
            &json!({"streams":[]}),
            None,
        )
        .unwrap();
        assert!(a
            .windows(2)
            .any(|w| w == [OsString::from("-crf"), OsString::from("20")]));
        assert!(a.contains(&OsString::from("/tmp/input with spaces.mp4")));
    }
}
