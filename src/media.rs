use crate::{process, Error, Result};
use serde_json::Value;
use std::{ffi::OsString, fs::File, path::Path};

pub fn args(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}
pub fn probe(path: &Path, log: &File) -> Result<Value> {
    let mut a = args(&[
        "-v",
        "error",
        "-show_format",
        "-show_streams",
        "-show_chapters",
        "-of",
        "json",
    ]);
    a.push(path.as_os_str().to_owned());
    let data = process::run("ffprobe", &a, None, log)
        .map_err(|e| Error::new(if e.code == 4 { 2 } else { e.code }, e.message))?;
    serde_json::from_slice(&data).map_err(|e| Error::new(2, format!("Invalid ffprobe output: {e}")))
}
pub fn streams(v: &Value) -> Result<&Vec<Value>> {
    v["streams"]
        .as_array()
        .ok_or_else(|| Error::new(2, "No stream inventory"))
}
pub fn duration(v: &Value) -> Result<f64> {
    v["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n > 0.0)
        .ok_or_else(|| Error::new(2, "A finite positive duration is required"))
}
pub fn video(v: &Value) -> Result<&Value> {
    let list: Vec<_> = streams(v)?
        .iter()
        .filter(|s| s["codec_type"] == "video")
        .collect();
    if list.len() != 1 {
        return Err(Error::new(2, "Exactly one video stream is supported; attached cover art/multiple video tracks are not supported yet"));
    }
    Ok(list[0])
}
pub fn check(v: &Value, compatible: bool) -> Result<()> {
    duration(v)?;
    let s = video(v)?;
    if !matches!(s["pix_fmt"].as_str(), Some("yuv420p" | "yuvj420p")) {
        return Err(Error::new(2, "This release supports 8-bit 4:2:0 SDR video only; HDR/high-bit-depth/chroma conversion is not supported"));
    }
    if !matches!(
        s["field_order"].as_str(),
        None | Some("unknown" | "progressive")
    ) {
        return Err(Error::new(2, "Interlaced video is not supported yet"));
    }
    if matches!(
        s["color_transfer"].as_str(),
        Some("smpte2084" | "arib-std-b67")
    ) || s["color_primaries"] == "bt2020"
        || s["side_data_list"].as_array().is_some_and(|a| {
            a.iter().any(|x| {
                x["side_data_type"].as_str().is_some_and(|t| {
                    t.contains("DOVI") || t.contains("Mastering") || t.contains("Content light")
                })
            })
        })
    {
        return Err(Error::new(2, "HDR is not supported yet"));
    }
    if s["tags"]["rotate"].as_str().is_some_and(|r| r != "0")
        || s["side_data_list"].as_array().is_some_and(|a| {
            a.iter()
                .any(|x| x["rotation"].as_i64().is_some_and(|r| r != 0))
        })
    {
        return Err(Error::new(
            2,
            "Rotated video is not supported yet; normalize its orientation first",
        ));
    }
    if s["width"].as_u64().is_none_or(|w| w % 2 != 0)
        || s["height"].as_u64().is_none_or(|h| h % 2 != 0)
    {
        return Err(Error::new(2, "Even video dimensions are required"));
    }
    for stream in streams(v)? {
        let kind = stream["codec_type"].as_str().unwrap_or("unknown");
        let codec = stream["codec_name"].as_str().unwrap_or("unknown");
        let supported = if compatible {
            kind == "video" || (kind == "audio" && matches!(codec, "aac" | "mp3" | "ac3" | "eac3"))
        } else {
            kind == "video"
                || (kind == "audio"
                    && matches!(
                        codec,
                        "aac"
                            | "mp3"
                            | "ac3"
                            | "eac3"
                            | "opus"
                            | "vorbis"
                            | "flac"
                            | "pcm_s16le"
                            | "pcm_s24le"
                            | "dts"
                            | "truehd"
                    ))
                || (kind == "subtitle"
                    && matches!(
                        codec,
                        "subrip" | "ass" | "ssa" | "hdmv_pgs_subtitle" | "dvd_subtitle"
                    ))
                || kind == "attachment"
        };
        if !supported {
            return Err(Error::new(2, format!("Cannot preserve {kind} stream ({codec}) with this profile; try modern or remux explicitly")));
        }
    }
    Ok(())
}

pub fn validate(before: &Value, after: &Value, mp4: bool) -> Result<Vec<String>> {
    let mut notices = Vec::new();
    let fail = |s: &str| Error::new(5, s);
    if (duration(before)? - duration(after)?).abs() > (duration(before)? * 0.001).max(0.25) {
        return Err(fail("Output duration differs from input"));
    }
    let a = streams(before)?;
    let b = streams(after)?;
    if a.len() != b.len() {
        return Err(fail("Stream count changed"));
    }
    for (a, b) in a.iter().zip(b) {
        if a["codec_type"] != b["codec_type"] {
            return Err(fail("Stream order/type changed"));
        }
        if a["codec_type"] == "video" {
            for key in [
                "width",
                "height",
                "color_transfer",
                "color_primaries",
                "color_space",
                "sample_aspect_ratio",
            ] {
                if !a[key].is_null() && a[key] != b[key] {
                    return Err(fail(&format!("Video {key} changed")));
                }
            }
        } else if a["codec_name"] != b["codec_name"] {
            return Err(fail("A copied stream's codec changed"));
        }
        if a["codec_type"] == "audio"
            && (a["channels"] != b["channels"] || a["sample_rate"] != b["sample_rate"])
        {
            return Err(fail("Audio layout/rate changed"));
        }
        if let Some(tags) = a["tags"].as_object() {
            for key in ["language", "title"] {
                if tags.contains_key(key) && a["tags"][key] != b["tags"][key] {
                    // Matroska omits the explicit ISO 639-2 "und" tag because
                    // an absent language tag has the same unspecified meaning.
                    if key == "language"
                        && a["tags"][key].as_str() == Some("und")
                        && b["tags"][key].is_null()
                    {
                        continue;
                    }
                    return Err(fail("Stream language/title changed"));
                }
            }
        }
        if a["disposition"] != b["disposition"] {
            // MP4 marks the only track of a type as enabled/default, even when
            // FFmpeg was explicitly given -disposition 0. Permit only that
            // container-imposed change; forced and accessibility flags stay exact.
            let mut expected = a["disposition"].clone();
            let single = streams(before)?
                .iter()
                .filter(|s| s["codec_type"] == a["codec_type"])
                .count()
                == 1;
            if mp4 && single && a["disposition"]["default"] == 0 && b["disposition"]["default"] == 1
            {
                expected["default"] = serde_json::json!(1);
            }
            if expected != b["disposition"] {
                return Err(fail("Stream dispositions changed"));
            }
            notices.push(format!(
                "MP4 marks the sole {} track as default",
                a["codec_type"].as_str().unwrap_or("unknown")
            ));
        }
    }
    let empty = Vec::new();
    let ac = before["chapters"].as_array().unwrap_or(&empty);
    let bc = after["chapters"].as_array().unwrap_or(&empty);
    if ac.len() != bc.len() {
        return Err(fail("Chapter count changed"));
    }
    for (a, b) in ac.iter().zip(bc) {
        for k in ["start_time", "end_time"] {
            let n = |v: &Value| v[k].as_str().and_then(|s| s.parse::<f64>().ok());
            if !matches!((n(a), n(b)), (Some(x),Some(y)) if (x-y).abs() <= 0.01) {
                return Err(fail("Chapter timing changed"));
            }
        }
        if a["tags"] != b["tags"] {
            return Err(fail("Chapter tags changed"));
        }
    }
    Ok(notices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> Value {
        json!({"format":{"duration":"2.0"},"chapters":[],"streams":[{"codec_type":"video","codec_name":"h264","width":320,"height":240,"pix_fmt":"yuv420p","field_order":"progressive","disposition":{"default":0,"forced":0}}]})
    }
    #[test]
    fn rejects_hdr_and_interlacing() {
        let mut v = fixture();
        assert!(check(&v, false).is_ok());
        v["streams"][0]["color_transfer"] = json!("smpte2084");
        assert!(check(&v, false).is_err());
        v["streams"][0]["color_transfer"] = Value::Null;
        v["streams"][0]["field_order"] = json!("tt");
        assert!(check(&v, false).is_err());
    }
    #[test]
    fn validates_dimensions_duration_and_flags() {
        let before = fixture();
        let mut after = before.clone();
        after["streams"][0]["width"] = json!(640);
        assert!(validate(&before, &after, false).is_err());
        after = before.clone();
        after["format"]["duration"] = json!("3.0");
        assert!(validate(&before, &after, false).is_err());
        after = before.clone();
        after["streams"][0]["disposition"]["forced"] = json!(1);
        assert!(validate(&before, &after, true).is_err());
        after = before.clone();
        after["streams"][0]["disposition"]["default"] = json!(1);
        assert!(validate(&before, &after, false).is_err());
        assert_eq!(validate(&before, &after, true).unwrap().len(), 1);
    }
    #[test]
    fn rejects_missing_and_changed_tracks() {
        let mut before = fixture();
        before["streams"].as_array_mut().unwrap().push(json!({"codec_type":"audio","codec_name":"aac","channels":6,"sample_rate":"48000","disposition":{}}));
        let mut after = before.clone();
        after["streams"][1]["channels"] = json!(2);
        assert!(validate(&before, &after, false).is_err());
        after = before.clone();
        after["streams"].as_array_mut().unwrap().pop();
        assert!(validate(&before, &after, false).is_err());
    }
}
