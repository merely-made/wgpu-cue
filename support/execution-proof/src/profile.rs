// SPDX-License-Identifier: MPL-2.0
//! Private diagnostic sink. No filesystem policy belongs to the adapter.
use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};
pub fn install() -> Result<(), Box<dyn std::error::Error>> {
    let bundled = std::env::current_exe()?
        .parent()
        .ok_or("executable parent")?
        .join("../Resources/profile-directory.txt");
    let directory = match std::env::var_os("CUE_SCENE_PROFILE_DIR") {
        Some(directory) => PathBuf::from(directory),
        None if bundled.is_file() => PathBuf::from(std::fs::read_to_string(bundled)?.trim()),
        None => return Ok(()),
    };
    install_at(directory)
}
pub fn install_at(directory: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    if !directory.is_absolute() || !directory.is_dir() {
        return Err("CUE_SCENE_PROFILE_DIR must name an existing absolute directory".into());
    }
    let pid = std::process::id();
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(directory.join(format!("stages-{pid}.jsonl")))?;
    let state = Arc::new(Mutex::new((file, 0u64)));
    wgpu_cue_netrender::scene_profile::install(Arc::new(move |sample| {
        let mut state = state.lock().expect("profile sink lock");
        assert!(state.1 < 20_000, "profile record budget exceeded");
        let record = serde_json::json!({"pid":pid,"sequence":state.1,
            "thread":format!("{:?}", std::thread::current().id()),
            "stage":sample.stage,"elapsed_ns":sample.elapsed.as_nanos() as u64,"bytes":sample.bytes});
        let mut line = serde_json::to_vec(&record).expect("profile record");
        line.push(b'\n');
        state.0.write_all(&line).expect("profile write");
        state.1 += 1;
    }))?;
    Ok(())
}
