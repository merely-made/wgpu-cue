// SPDX-License-Identifier: MPL-2.0
use super::AnyResult;
use std::path::PathBuf;
pub(super) struct Config {
    pub(super) output: PathBuf,
    pub(super) helper: PathBuf,
    pub(super) local: bool,
    pub(super) byte_snapshots: bool,
    pub(super) phase: f64,
    pub(super) warmup: f64,
    pub(super) font: String,
}
impl Config {
    pub(super) fn parse() -> AnyResult<Self> {
        let mut launch: Vec<String> = std::env::args().skip(1).collect();
        if launch.is_empty() {
            let config = std::env::current_exe()?
                .parent()
                .ok_or("executable parent")?
                .join("../Resources/arguments.json");
            launch = serde_json::from_slice(&std::fs::read(config)?)?;
        }
        let mut args = launch.into_iter();
        let mut output = None;
        let mut helper = None;
        let mut local = false;
        let mut byte_snapshots = false;
        let mut phase = 15.;
        let mut warmup = 3.;
        let mut font = "sans-serif".to_owned();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--output" => output = Some(PathBuf::from(args.next().ok_or("output missing")?)),
                "--helper" => helper = Some(PathBuf::from(args.next().ok_or("helper missing")?)),
                "--local" => local = true,
                "--byte-snapshots" => byte_snapshots = true,
                "--font" => font = args.next().ok_or("font missing")?,
                "--phase-seconds" => phase = args.next().ok_or("phase missing")?.parse()?,
                "--warmup-seconds" => warmup = args.next().ok_or("warmup missing")?.parse()?,
                _ => return Err(format!("unknown argument {arg}").into()),
            }
        }
        if !["Arial", "sans-serif", "monospace"].contains(&font.as_str()) {
            return Err("unsupported fixture font family".into());
        }
        let output = output.ok_or("--output absolute directory required")?;
        let helper = helper.unwrap_or(std::env::current_exe()?.with_file_name(if cfg!(windows) {
            "cue-execution-proof.exe"
        } else {
            "cue-execution-proof"
        }));
        if !output.is_absolute()
            || !helper.is_absolute()
            || !(5.0..=120.0).contains(&phase)
            || !(1.0..=30.0).contains(&warmup)
        {
            return Err(
                "absolute paths, phase 5..120 seconds and warmup 1..30 seconds required".into(),
            );
        }
        std::fs::create_dir_all(&output)?;
        if output.join("summary.json").exists() || output.join("frames.json").exists() {
            return Err("receipt already exists; choose a fresh run directory".into());
        }
        Ok(Self {
            output,
            helper,
            local,
            byte_snapshots,
            phase,
            warmup,
            font,
        })
    }
    pub(super) fn phase(&self, t: f64) -> &'static str {
        if t < self.warmup {
            "warmup"
        } else if t < self.warmup + self.phase {
            "steady"
        } else if t < self.warmup + 2. * self.phase {
            "loaded"
        } else if t < self.warmup + 2. * self.phase + 3. {
            "custody"
        } else {
            "drain"
        }
    }
    pub(super) fn end(&self) -> f64 {
        self.warmup + 2. * self.phase + 6.
    }
}
