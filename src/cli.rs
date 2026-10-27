use std::{ffi::OsString, path::PathBuf};

pub const HELP: &str = "vopt — smaller videos using FFmpeg\n\nUSAGE:\n  vopt <INPUT> [OPTIONS]\n  vopt inspect <INPUT> [--json]\n  vopt doctor [--json]\n\nOPTIONS:\n  -o, --output <PATH>          Output path (never overwrites)\n      --profile <PROFILE>     modern (AV1/MKV, default), compatible (H.264/MP4)\n      --effort <EFFORT>       quick, balanced (default), thorough\n      --quality <QUALITY>     high (default), strict\n      --dry-run               Show the plan without encoding\n      --json                  Machine-readable output; progress goes to stderr\n  -h, --help                  Show help\n  -V, --version               Show version\n\nExamples:\n  vopt holiday.mp4\n  vopt holiday.mp4 --profile compatible -o holiday-small.mp4\n  vopt recording.mkv --quality strict --effort thorough\n\nQuality is preset-based, not measured. Originals are always preserved.\nExit codes: 0 success, 2 invalid input/options, 3 dependency missing,\n4 encoding failed, 5 validation failed, 10 no smaller result, 130 cancelled.\n";

#[derive(Debug, Default)]
pub struct Cli {
    pub command: String,
    pub input: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub profile: String,
    pub effort: String,
    pub quality: String,
    pub dry_run: bool,
    pub json: bool,
}

impl Cli {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut c = Self {
            command: "optimize".into(),
            profile: "modern".into(),
            effort: "balanced".into(),
            quality: "high".into(),
            ..Self::default()
        };
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
                Some("--dry-run") => {
                    c.dry_run = true;
                    optimization_option = true;
                }
                Some("-o" | "--output") => {
                    c.output = Some(args.next().ok_or("--output requires a path")?.into());
                    optimization_option = true;
                }
                Some("--profile" | "--effort" | "--quality") => {
                    let key = arg.to_str().unwrap();
                    let value = args
                        .next()
                        .ok_or_else(|| format!("{key} requires a value"))?
                        .into_string()
                        .map_err(|_| "Option values must be UTF-8")?;
                    match key {
                        "--profile" => c.profile = value,
                        "--effort" => c.effort = value,
                        _ => c.quality = value,
                    }
                    optimization_option = true;
                }
                Some("doctor" | "inspect") if c.input.is_none() && c.command == "optimize" => {
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
        if !["modern", "compatible"].contains(&c.profile.as_str()) {
            return Err("--profile must be modern or compatible".into());
        }
        if !["quick", "balanced", "thorough"].contains(&c.effort.as_str()) {
            return Err("--effort must be quick, balanced, or thorough".into());
        }
        if !["high", "strict"].contains(&c.quality.as_str()) {
            return Err("--quality must be high or strict".into());
        }
        if c.command == "doctor" && c.input.is_some() {
            return Err("doctor takes no input".into());
        }
        if c.command != "doctor" && c.input.is_none() {
            return Err("An input is required; use --help".into());
        }
        if c.command != "optimize" && optimization_option {
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
