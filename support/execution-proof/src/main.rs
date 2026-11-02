// SPDX-License-Identifier: MPL-2.0
use cue_execution_proof::{Executor, Request, encode, read_frame, write_frame};
use std::io;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(all(feature = "metal-handoff", target_os = "macos"))]
    if std::env::args().any(|arg| arg == "--metal-handoff") {
        return cue_execution_proof::metal_handoff::helper();
    }
    #[cfg(feature = "cpu-raster")]
    if std::env::args().any(|arg| arg == "--raster") {
        return cue_execution_proof::raster::helper();
    }
    let hold = std::env::args().any(|arg| arg == "--hold");
    let mut executor = Executor::new()?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    while let Some(request) = read_frame::<Request>(&mut input)? {
        if hold {
            write_frame(&mut output, &encode(&executor.hold(request)?)?)?;
            loop {
                std::thread::park();
            }
        }
        for update in executor.execute(request)? {
            write_frame(&mut output, &encode(&update)?)?;
        }
    }
    Ok(())
}
