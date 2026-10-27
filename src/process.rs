use crate::{Error, Result};
use std::{
    ffi::OsString,
    fs::File,
    io::{BufRead, BufReader, Read},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};
pub static CANCELLED: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
extern "C" fn cancel(_: libc::c_int) {
    CANCELLED.store(true, Ordering::SeqCst);
}
pub fn install_signals() {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGINT, cancel as libc::sighandler_t);
        libc::signal(libc::SIGTERM, cancel as libc::sighandler_t);
    }
}

pub fn run(
    program: &str,
    args: &[OsString],
    progress: Option<f64>,
    diagnostic: &File,
) -> Result<Vec<u8>> {
    if CANCELLED.load(Ordering::SeqCst) {
        return Err(Error::new(130, "Cancelled"));
    }
    diagnostic.set_len(0).map_err(Error::io)?;
    {
        use std::io::{Seek, SeekFrom};
        diagnostic
            .try_clone()
            .map_err(Error::io)?
            .seek(SeekFrom::Start(0))
            .map_err(Error::io)?;
    }
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(diagnostic.try_clone().map_err(Error::io)?)
        .spawn()
        .map_err(|e| {
            Error::new(
                if e.kind() == std::io::ErrorKind::NotFound {
                    3
                } else {
                    4
                },
                format!("Cannot start {program}: {e}"),
            )
        })?;
    let stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut reader = BufReader::new(stdout);
        if let Some(duration) = progress {
            let mut line = String::new();
            let start = std::time::Instant::now();
            while reader.read_line(&mut line)? > 0 {
                if let Some(n) = line
                    .trim()
                    .strip_prefix("out_time_us=")
                    .and_then(|n| n.parse::<f64>().ok())
                {
                    let completed = (n / 1_000_000.0).max(0.0);
                    let speed = completed / start.elapsed().as_secs_f64().max(0.001);
                    if speed > 0.0 {
                        eprint!(
                            "\rEncoding: {:5.1}% | {:.2}x | ETA ~{:.0}s   ",
                            (completed / duration * 100.0).clamp(0.0, 100.0),
                            speed,
                            ((duration - completed).max(0.0) / speed)
                        );
                    }
                }
                line.clear();
            }
            Ok(Vec::new())
        } else {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes)?;
            Ok(bytes)
        }
    });
    let status = loop {
        if CANCELLED.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            if progress.is_some() {
                eprintln!();
            }
            return Err(Error::new(130, "Cancelled; original preserved"));
        }
        if let Some(status) = child.try_wait().map_err(Error::io)? {
            break status;
        }
        thread::sleep(Duration::from_millis(100));
    };
    let bytes = reader
        .join()
        .map_err(|_| Error::new(4, "Output reader failed"))?
        .map_err(Error::io)?;
    if progress.is_some() {
        eprintln!();
    }
    if !status.success() {
        // Bounded diagnostic tail; stderr is written to disk rather than buffered in RAM.
        let mut f = diagnostic.try_clone().map_err(Error::io)?;
        use std::io::{Seek, SeekFrom};
        let len = f.metadata().map_err(Error::io)?.len();
        f.seek(SeekFrom::Start(len.saturating_sub(8192)))
            .map_err(Error::io)?;
        let mut tail = String::new();
        f.read_to_string(&mut tail).map_err(Error::io)?;
        return Err(Error::new(
            4,
            format!("{program} failed ({status}):\n{tail}"),
        ));
    }
    Ok(bytes)
}
