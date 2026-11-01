// SPDX-License-Identifier: MPL-2.0
use cue_execution_proof::{Executor, Request, encode, read_frame, write_frame};
use std::io;

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
