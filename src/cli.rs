use std::{ffi::OsString, path::PathBuf};

pub const HELP: &str = "vopt — smaller videos using FFmpeg\n\nUSAGE:\n  vopt <INPUT> [OPTIONS]\n  vopt inspect <INPUT> [--json]\n  vopt doctor [--json]\n  vopt batch <DIRECTORY> [--recursive] [OPTIONS]\n\nOPTIONS:\n  -o, --output <PATH>          Output path (never overwrites)\n      --profile <PROFILE>     modern (AV1/MKV, default), compatible (H.264/MP4), hevc\n      --effort <EFFORT>       quick, balanced (default), thorough\n      --quality <QUALITY>     high (default), strict\n      --dry-run               Show the plan without encoding\n      --auto                  Select CRF using VMAF sample comparisons\n      --analyze               Sample and estimate without a full encode\n      --verify                Measure full-output VMAF before publishing\n      --preview <DIR>         Save source and encoded sample clips\n      --content <TYPE>        mixed (default), screen, camera\n      --threads <N>           Encoder/quality thread limit (default 4)\n      --recursive             Include subdirectories in batch\n      --config <FILE>         Load JSON defaults; CLI options take priority\n      --json                  Machine-readable output; progress goes to stderr\n  -h, --help                  Show help\n  -V, --version               Show version\n\nExamples:\n  vopt holiday.mp4\n  vopt holiday.mp4 --profile compatible -o holiday-small.mp4\n  vopt recording.mkv --quality strict --effort thorough\n\nDefault quality is preset-based. Use --auto and --verify for VMAF comparisons.\nExit codes: 0 success, 2 invalid input/options, 3 dependency missing,\n4 encoding failed, 5 validation failed, 10 no smaller result, 130 cancelled.\n";

#[derive(Clone, Debug, Default)]
pub struct Cli {
    pub command: String,
    pub input: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub profile: String,
    pub effort: String,
    pub quality: String,
    pub dry_run: bool,
    pub json: bool,
    pub auto: bool,
    pub analyze: bool,
    pub verify: bool,
    pub content: String,
    pub threads: u32,
    pub preview: Option<PathBuf>,
    pub recursive: bool,
}

impl Cli {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut c = Self {
            command: "optimize".into(),
            profile: "modern".into(),
            effort: "balanced".into(),
            quality: "high".into(),
            content: "mixed".into(),
            threads: 4,
            ..Self::default()
        };
        let args: Vec<OsString> = args.into_iter().collect();
        let mut scan = args.iter();
        let mut config = None;
        while let Some(arg) = scan.next() {
            match arg.to_str() {
                Some("--") => break,
                Some("--config") => {
                    if config.is_some() {
                        return Err("--config can be supplied only once".into());
                    }
                    config = Some(scan.next().ok_or("--config requires a file")?.clone());
                }
                Some(
                    "--profile" | "--effort" | "--quality" | "--content" | "--threads"
                    | "--preview" | "--output" | "-o",
                ) => {
                    scan.next();
                }
                _ => {}
            }
        }
        if let Some(path) = config {
            let path = PathBuf::from(path);
            let data = std::fs::read(&path)
                .map_err(|e| format!("Cannot read config {}: {e}", path.display()))?;
            let value: serde_json::Value =
                serde_json::from_slice(&data).map_err(|e| format!("Invalid config JSON: {e}"))?;
            let settings = value.as_object().ok_or("Config must be a JSON object")?;
            for (key, value) in settings {
                match key.as_str() {
                    "profile" | "effort" | "quality" | "content" => {
                        let value = value
                            .as_str()
                            .ok_or_else(|| format!("Config {key} must be a string"))?
                            .to_owned();
                        match key.as_str() {
                            "profile" => c.profile = value,
                            "effort" => c.effort = value,
                            "quality" => c.quality = value,
                            _ => c.content = value,
                        }
                    }
                    "threads" => {
                        let n = value
                            .as_u64()
                            .ok_or("Config threads must be between 1 and 64")?;
                        if !(1..=64).contains(&n) {
                            return Err("Config threads must be between 1 and 64".into());
                        }
                        c.threads = n as u32;
                    }
                    "auto" | "verify" => {
                        let b = value
                            .as_bool()
                            .ok_or_else(|| format!("Config {key} must be a boolean"))?;
                        if key == "auto" {
                            c.auto = b;
                        } else {
                            c.verify = b;
                        }
                    }
                    _ => return Err(format!("Unknown config key {key}")),
                }
            }
        }
        let mut args = args.into_iter().peekable();
        let mut positional_only = false;
        let mut optimization_option = false;
        while let Some(arg) = args.next() {
            if positional_only {
                if c.input.replace(arg.into()).is_some() {
                    return Err("Only one input is supported".into());
                }
                continue;
            }
            match arg.to_str() {
                Some("--") => positional_only = true,
                Some("-h" | "--help") => {
                    c.command = "help".into();
                    return Ok(c);
                }
                Some("-V" | "--version") => {
                    c.command = "version".into();
                    return Ok(c);
                }
                Some("--json") => c.json = true,
                Some("--config") => {
                    args.next().ok_or("--config requires a file")?;
                }
                Some("--recursive") => c.recursive = true,
                Some("--auto" | "--analyze" | "--verify") => {
                    match arg.to_str().unwrap() {
                        "--auto" => c.auto = true,
                        "--analyze" => {
                            c.analyze = true;
                            c.auto = true;
                        }
                        _ => c.verify = true,
                    }
                    optimization_option = true;
                }
                Some("--preview") => {
                    c.preview = Some(args.next().ok_or("--preview requires a directory")?.into());
                    c.auto = true;
                    optimization_option = true;
                }
                Some("--threads") => {
                    let n = args
                        .next()
                        .ok_or("--threads requires a number")?
                        .into_string()
                        .map_err(|_| "Threads must be a number")?;
                    c.threads = n.parse().map_err(|_| "Threads must be between 1 and 64")?;
                    if !(1..=64).contains(&c.threads) {
                        return Err("Threads must be between 1 and 64".into());
                    }
                    optimization_option = true;
                }
                Some("--dry-run") => {
                    c.dry_run = true;
                    optimization_option = true;
                }
                Some("-o" | "--output") => {
                    c.output = Some(args.next().ok_or("--output requires a path")?.into());
                    optimization_option = true;
                }
                Some("--profile" | "--effort" | "--quality" | "--content") => {
                    let key = arg.to_str().unwrap();
                    let value = args
                        .next()
                        .ok_or_else(|| format!("{key} requires a value"))?
                        .into_string()
                        .map_err(|_| "Option values must be UTF-8")?;
                    match key {
                        "--profile" => c.profile = value,
                        "--effort" => c.effort = value,
                        "--quality" => c.quality = value,
                        _ => c.content = value,
                    }
                    optimization_option = true;
                }
                Some("doctor" | "inspect" | "batch")
                    if c.input.is_none() && c.command == "optimize" =>
                {
                    c.command = arg.to_str().unwrap().into()
                }
                Some(s) if s.starts_with('-') => {
                    return Err(format!(
                    "Unknown option {s}; use --help (or -- before a filename starting with '-')"
                ))
                }
                _ => {
                    if c.input.replace(arg.into()).is_some() {
                        return Err("Only one input is supported".into());
                    }
                }
            }
        }
        if !["modern", "compatible", "hevc"].contains(&c.profile.as_str()) {
            return Err("--profile must be modern, compatible, or hevc".into());
        }
        if !["quick", "balanced", "thorough"].contains(&c.effort.as_str()) {
            return Err("--effort must be quick, balanced, or thorough".into());
        }
        if !["high", "strict"].contains(&c.quality.as_str()) {
            return Err("--quality must be high or strict".into());
        }
        if !["screen", "camera", "mixed"].contains(&c.content.as_str()) {
            return Err("--content must be screen, camera, or mixed".into());
        }
        if c.dry_run && (c.analyze || c.preview.is_some()) {
            return Err("--dry-run cannot be combined with --analyze or --preview".into());
        }
        if c.analyze && c.verify {
            return Err("--verify requires a full encode".into());
        }
        if c.command == "doctor" && c.input.is_some() {
            return Err("doctor takes no input".into());
        }
        if c.command != "doctor" && c.input.is_none() {
            return Err("An input is required; use --help".into());
        }
        if c.recursive && c.command != "batch" {
            return Err("--recursive requires batch".into());
        }
        if c.command == "batch" && (c.output.is_some() || c.preview.is_some()) {
            return Err("batch uses automatic output names and does not support --preview".into());
        }
        if c.command != "optimize" && c.command != "batch" && optimization_option {
            return Err("Encoding options only apply to optimization".into());
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(a: &[&str]) -> Result<Cli, String> {
        Cli::parse(a.iter().map(OsString::from))
    }
    #[test]
    fn simple_defaults() {
        let c = parse(&["a file.mp4"]).unwrap();
        assert_eq!(c.profile, "modern");
        assert_eq!(c.input.unwrap(), PathBuf::from("a file.mp4"));
    }
    #[test]
    fn rejects_conflicts() {
        for args in [
            vec!["doctor", "x"],
            vec!["inspect", "x", "--dry-run"],
            vec!["x", "--quality", "magic"],
            vec!["x", "y"],
        ] {
            assert!(parse(&args).is_err());
        }
    }
    #[test]
    fn escaped_filename() {
        assert_eq!(
            parse(&["--", "--help"]).unwrap().input.unwrap(),
            PathBuf::from("--help")
        );
    }
}
